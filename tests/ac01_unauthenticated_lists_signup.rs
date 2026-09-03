//! AC1 — Given a fresh server with an empty data directory, When an
//! unauthenticated client sends `initialize` then `tools/list`, Then the
//! response carries `MCP-Protocol-Version` and lists exactly one tool,
//! `signup`.

mod common;
use common::{McpClient, TestServer};
use serde_json::Value;

#[tokio::test]
async fn unauthenticated_tools_list_is_signup_only() {
    let server = TestServer::start().await;
    let client = McpClient::new(&server.base_url);

    let init_resp = client
        .post_raw(serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2026-07-28",
                "capabilities": {},
                "clientInfo": {"name": "test", "version": "0.1"}
            }
        }))
        .await;
    assert_eq!(
        init_resp
            .headers()
            .get("MCP-Protocol-Version")
            .and_then(|v| v.to_str().ok()),
        Some("2026-07-28"),
        "every response must carry MCP-Protocol-Version"
    );
    let init_body: Value = init_resp.json().await.expect("parse initialize");
    assert!(
        init_body.get("error").is_none(),
        "initialize should not error: {init_body:?}"
    );

    let tools = client
        .tools_list()
        .await
        .expect("tools/list should succeed unauthenticated");
    let tool_names: Vec<&str> = tools["tools"]
        .as_array()
        .expect("tools array")
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        tool_names,
        vec!["signup"],
        "unauthenticated tools/list must list exactly signup"
    );
}
