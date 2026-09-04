//! AC1 — Given a fresh server with an empty data directory, When an
//! unauthenticated client sends `initialize` then `tools/list`, Then the
//! response carries `MCP-Protocol-Version` and lists `signup` alongside
//! the discoverable `host.*` control plane (PRD-mcphost-session-key
//! requirement 1 widened this from `signup` alone), and no `admin.*` tool
//! or namespaced tenant tool.

mod common;
use common::{McpClient, TestServer};
use rmcp::model::ProtocolVersion;
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
    // PRD-mcphost-protocol-compat requirement 6: mcphost advertises the
    // version it actually negotiates (`rmcp::model::ProtocolVersion::LATEST`),
    // not the `2026-07-28` literal this assertion used to hardcode -- that
    // literal *was* defect B (see AC9/AC10 in compat_ac08_ac09_ac10_advertised_version.rs).
    assert_eq!(
        init_resp
            .headers()
            .get("MCP-Protocol-Version")
            .and_then(|v| v.to_str().ok()),
        Some(ProtocolVersion::LATEST.as_str()),
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
    assert!(
        tool_names.contains(&"signup"),
        "unauthenticated tools/list must list signup: {tool_names:?}"
    );
    for expected in [
        "host.whoami",
        "host.tool_publish",
        "host.tool_list",
        "host.tool_remove",
        "host.tool_logs",
        "host.tool_test",
        "host.tool_call",
        "host.usage",
        "host.secret_set",
        "host.secret_list",
        "host.registry_publish",
    ] {
        assert!(
            tool_names.contains(&expected),
            "unauthenticated tools/list must list {expected}: {tool_names:?}"
        );
    }
    assert!(
        !tool_names.iter().any(|n| n.starts_with("admin.")),
        "unauthenticated tools/list must not list any admin.* tool: {tool_names:?}"
    );
    assert_eq!(
        tool_names.len(),
        12,
        "signup + the ten host.* tools + host.tool_call: {tool_names:?}"
    );
}
