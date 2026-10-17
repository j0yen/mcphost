//! PRD-mcphost-publish-visibility-and-tags AC3 (P0) -- Given a python tool
//! whose source raises on every call, When published with
//! `visibility: "public"`, Then the error is `untested_tool_cannot_be_public`
//! with `data.verdict` and `data.next = "host.tool_test"`, and
//! `host.tool_list` shows the tool `private`.

use crate::common;
use common::{TestServer, extract_structured, python_kind_registry, signup};
use mcphost::sandbox;
use serde_json::json;

#[tokio::test]
async fn pubvis_ac03() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs_dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (_ns, key) = signup(&server.base_url, "Pubvis AC3 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    let err = client
        .tools_call(
            "host.tool_publish",
            json!({
                "name": "always_raises",
                "kind": "python",
                "spec": {"source": "def main(args):\n    raise RuntimeError(\"boom\")\n"},
                "visibility": "public",
            }),
        )
        .await
        .expect_err("a tool that raises on every call must not publish public");
    assert_eq!(err.error_code.as_deref(), Some("untested_tool_cannot_be_public"), "{err:?}");
    assert!(err.data["verdict"].is_string(), "data.verdict: {err:?}");
    assert_eq!(err.data["next"], json!("host.tool_test"), "{err:?}");

    let list = extract_structured(&client.tools_call("host.tool_list", json!({})).await.expect("tool_list"));
    let tool = list["tools"]
        .as_array()
        .expect("tools array")
        .iter()
        .find(|t| t["name"].as_str().is_some_and(|n| n.ends_with("always_raises")))
        .unwrap_or_else(|| panic!("tool stored (private) after the refusal: {list}"));
    assert_eq!(tool["visibility"], json!("private"), "{list}");
}
