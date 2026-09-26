//! PRD-mcphost-first-publish-real-kind
//! AC5 (P0) -- Given a tenant with one echo tool and one python tool, When
//! `tools/list` is read, Then the echo entry has `stub: true` and the
//! python entry has no `stub` key.

use crate::common;
use common::{TestServer, python_kind_registry, signup};
use mcphost::sandbox;
use serde_json::json;

#[tokio::test]
async fn echo_tool_carries_stub_true_python_tool_has_no_stub_key() {
    // Publishing the python tool reaches `Kind::validate_async`'s sandboxed
    // AST check, same as every other test that publishes a real python
    // spec.
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs_dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (ns, key) = signup(&server.base_url, "AC5 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "greeter", "kind": "echo", "spec": {"schema": {"type": "object"}}}),
        )
        .await
        .expect("echo publish");
    client
        .tools_call(
            "host.tool_publish",
            json!({
                "name": "stats",
                "kind": "python",
                "spec": {"source": "def main(args):\n    return {}\n"},
            }),
        )
        .await
        .expect("python publish");

    let listed = client.tools_list().await.expect("tools/list");
    let tools = listed["tools"].as_array().expect("tools array");

    let echo_tool = tools
        .iter()
        .find(|t| t["name"] == json!(format!("{ns}.greeter")))
        .expect("echo tool listed");
    assert_eq!(
        echo_tool["_meta"]["stub"],
        json!(true),
        "echo tool must carry stub: true: {echo_tool}"
    );

    let python_tool = tools
        .iter()
        .find(|t| t["name"] == json!(format!("{ns}.stats")))
        .expect("python tool listed");
    assert!(
        python_tool["_meta"].get("stub").is_none(),
        "python tool must have no stub key at all: {python_tool}"
    );
}
