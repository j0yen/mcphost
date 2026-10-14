//! PRD-mcphost-publish-time-tool-lint
//! AC2 (P0) -- Given a source using `mcphost.progress(...)` on line 9 with no
//! import of `mcphost`, When published, Then `tool_lint` has one finding
//! `{rule: mcphost_not_imported, line: 9, fix: "add `import mcphost` at line 1"}`
//! and the publish succeeds; Given the same source with `import mcphost`,
//! Then `tool_lint` is `[]`.

use crate::common;
use common::{McpClient, TestServer, extract_structured, python_kind_registry, signup};
use serde_json::{Value, json};

/// `mcphost.progress` sits on line 9 of both variants (the import variant
/// swaps the blank line 1 for `import mcphost`).
fn source(with_import: bool) -> String {
    let first = if with_import { "import mcphost" } else { "" };
    format!(
        "{first}\n\n\n# a tool that reports progress\nimport time\n\n\ndef main(args):\n    mcphost.progress(50, \"halfway\")\n    return {{\"ok\": True}}\n"
    )
}

async fn publish(client: &McpClient, name: &str, source: String) -> Value {
    let result = client
        .tools_call("host.tool_publish", json!({"name": name, "kind": "python", "spec": {"source": source}}))
        .await
        .unwrap_or_else(|e| panic!("publish must succeed despite a lint: {} {}", e.code, e.message));
    extract_structured(&result)
}

#[tokio::test]
async fn missing_import_reports_first_use_line_and_publishes() {
    let envs_dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (_ns, key) = signup(&server.base_url, "Toollint AC2 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let published = publish(&client, "noimport", source(false)).await;
    assert_eq!(published["kind"], json!("python"), "the publish must succeed: {published}");
    let lint = published["tool_lint"].as_array().expect("tool_lint is an array");
    assert_eq!(lint.len(), 1, "exactly one finding: {lint:?}");
    assert_eq!(lint[0]["rule"], json!("mcphost_not_imported"));
    assert_eq!(lint[0]["line"], json!(9));
    assert_eq!(lint[0]["fix"], json!("add `import mcphost` at line 1"));
    assert!(lint[0]["source_line"].as_str().unwrap().contains("mcphost.progress"));
    assert!(lint[0]["message"].as_str().unwrap().contains("line 9"));
}

#[tokio::test]
async fn imported_mcphost_is_clean() {
    let envs_dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (_ns, key) = signup(&server.base_url, "Toollint AC2 Clean Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let published = publish(&client, "withimport", source(true)).await;
    assert_eq!(published["tool_lint"], json!([]), "an imported mcphost lints clean: {published}");
}
