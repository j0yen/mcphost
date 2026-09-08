//! AC6 (P1) -- Given a published tool, When `tools/list` is read, Then its
//! metadata includes `kind`.

mod common;
use common::{TestServer, signup};
use mcphost::kinds::KindRegistry;
use serde_json::json;

#[tokio::test]
async fn tools_list_meta_reports_kind() {
    let server = TestServer::start_with_kinds(KindRegistry::with_builtin()).await;
    let (ns, key) = signup(&server.base_url, "Kind Honor AC6").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call(
            "host.tool_publish",
            json!({
                "name": "echo_tool",
                "kind": "echo",
                "spec": {"schema": {"type": "object"}},
            }),
        )
        .await
        .expect("publish echo tool");

    let result = client.tools_list().await.expect("tools/list");
    let tools = result["tools"].as_array().expect("tools array");
    let qualified_name = format!("{ns}.echo_tool");
    let tool = tools
        .iter()
        .find(|t| t["name"] == json!(qualified_name))
        .expect("published tool is listed");
    assert_eq!(
        tool["_meta"]["kind"],
        json!("echo"),
        "tool metadata must report its kind: {tool:?}"
    );
}
