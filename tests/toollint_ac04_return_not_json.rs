//! PRD-mcphost-publish-time-tool-lint
//! AC4 (P0) -- Given `resp = requests.get(url)` then `return resp` in `main`,
//! When published, Then one `return_not_json` finding at the `return` line
//! with fix `return resp.json()`; Given `return resp.json()`, Then no finding.

use crate::common;
use common::{McpClient, TestServer, extract_structured, python_kind_registry, signup};
use serde_json::{Value, json};

fn source(ret: &str) -> String {
    format!(
        "import requests\n\n\ndef main(args):\n    url = args.get(\"url\", \"https://example.com\")\n    resp = requests.get(url)\n    resp.raise_for_status()\n    {ret}\n"
    )
}

async fn publish(client: &McpClient, name: &str, source: String) -> Value {
    let result = client
        .tools_call(
            "host.tool_publish",
            json!({"name": name, "kind": "python", "spec": {"source": source, "requirements": ["requests"]}}),
        )
        .await
        .unwrap_or_else(|e| panic!("publish must succeed despite a lint: {} {}", e.code, e.message));
    extract_structured(&result)
}

#[tokio::test]
async fn non_json_return_reports_return_line_and_per_kind_fix() {
    let envs_dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (_ns, key) = signup(&server.base_url, "Toollint AC4 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let published = publish(&client, "returns_resp", source("return resp")).await;
    let lint = published["tool_lint"].as_array().expect("tool_lint is an array");
    assert_eq!(lint.len(), 1, "exactly one finding: {lint:?}");
    assert_eq!(lint[0]["rule"], json!("return_not_json"));
    assert_eq!(lint[0]["line"], json!(8), "the `return` line");
    assert_eq!(lint[0]["fix"], json!("return resp.json()"));

    let published = publish(&client, "returns_json", source("return resp.json()")).await;
    assert_eq!(published["tool_lint"], json!([]), "resp.json() lints clean: {published}");
}
