//! AC1 — Given a fresh server with an empty data directory, When an
//! unauthenticated client sends `initialize` then `tools/list`, Then the
//! response carries `MCP-Protocol-Version` and lists `signup`, and no
//! `admin.*` tool or namespaced tenant tool.
//!
//! PRD-mcphost-one-next-tool requirement 1 narrowed the anonymous listing
//! from the full `host.*`/`billing.*` control plane (PRD-mcphost-session-key
//! requirement 1's widening) down to a twelve-tool starter set -- the exact
//! contents and `signup`-first ordering are pinned by
//! `tests/nexttool_ac01_anonymous_starter_set.rs`; this file keeps its own
//! original initialize/protocol-version proof and just updates the stale
//! 147-tool expectation migration note (PRD-mcphost-one-next-tool's own
//! Migration/compatibility section).

use crate::common;
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
    assert!(
        !tool_names.iter().any(|n| n.starts_with("admin.")),
        "unauthenticated tools/list must not list any admin.* tool: {tool_names:?}"
    );
    assert_eq!(
        tool_names.len(),
        12,
        "PRD-mcphost-one-next-tool requirement 1 (AC1): signup plus the eleven-tool starter \
         set, not the full host.*/billing.* control plane: {tool_names:?}"
    );
}
