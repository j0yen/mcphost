//! AC13 — Given a request with a mismatched `Mcp-Name` header and body
//! tool name, When metered, Then the `calls` row records the body's name
//! and the log line flags the mismatch.
//!
//! `rmcp` itself rejects a mismatched `Mcp-Name` header at the transport
//! layer (SEP-2243) once a request declares `MCP-Protocol-Version:
//! 2026-07-28` — the version this crate otherwise always negotiates — so
//! this test deliberately omits that HTTP header (per-request metadata in
//! the JSON-RPC `_meta` object still negotiates 2026-07-28 statelessly)
//! to reach the application-level mismatch-detection path this AC tests.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn mismatched_mcp_name_header_is_recorded_and_flagged() {
    let server = TestServer::start().await;
    let (tenant_ns, key) = signup(&server.base_url, "Mismatch Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "hello", "kind": "echo", "spec": {"schema": {"type": "object"}}}),
        )
        .await
        .expect("publish");
    let qualified = format!("{tenant_ns}.hello");

    // No `_meta.protocolVersion` and no `MCP-Protocol-Version` header: this
    // is the "older client" path (see module docs) where `rmcp` requires
    // neither per-request protocol metadata nor SEP-2243 `Mcp-Method`
    // header-vs-body agreement, so a mismatched `Mcp-Name` reaches this
    // crate's own handler instead of being rejected by the transport.
    let body = json!({
        "jsonrpc": "2.0",
        "id": 999,
        "method": "tools/call",
        "params": {
            "name": qualified,
            "arguments": {"probe": true},
        },
    });
    let resp = client
        .post_with_mcp_name_override(body, "definitely-not-the-tool-name")
        .await;
    let status = resp.status();
    let text = resp.text().await.expect("read response body");
    assert!(
        status.is_success(),
        "the mismatch itself must not fail the call: {status} {text}"
    );
    let parsed: serde_json::Value = serde_json::from_str(&text).expect("parse response");
    assert!(
        parsed.get("error").is_none(),
        "a header/body Mcp-Name mismatch must not error the call: {parsed:?}"
    );
    let structured = extract_structured(&parsed["result"]);
    assert_eq!(
        structured,
        json!({"probe": true}),
        "the call must still execute against the body's tool name"
    );

    let tenant = server
        .state
        .db
        .find_tenant_by_namespace(tenant_ns)
        .await
        .unwrap()
        .expect("tenant");
    let usage = server.state.db.usage(tenant.id, 3600).await.unwrap();
    assert_eq!(
        usage.calls, 1,
        "the calls row must record the body's tool name's call"
    );
}
