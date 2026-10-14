//! AC7 (PRD-mcphost-quickstart-by-client) — a quickstart that detects a
//! surface writes it to a NULL `tenants.client_name`, and a later detection
//! does not overwrite it.

use crate::common;
use common::{TestServer, extract_structured};
use serde_json::json;

#[tokio::test]
async fn detected_surface_fills_a_null_client_name_once() {
    let server = TestServer::start().await;
    let key = "qsbc-ac07-key";
    let tenant = server
        .state
        .db
        .create_tenant("AC7".into(), "qsbcac07".into(), mcphost::auth::hash_key(key), None)
        .await
        .expect("create_tenant");
    assert!(tenant.client_name.is_none());

    // Raw requests with no `_meta` clientInfo (the harness client always
    // sends one, which the earlier attribution hook would capture first), so
    // detection falls through to the User-Agent.
    let http = reqwest::Client::new();
    for (ua, expect_detected) in [("windsurf/2.0", "windsurf"), ("cursor/1.0", "cursor")] {
        let resp = http
            .post(format!("{}/mcp", server.base_url))
            .header("Content-Type", "application/json")
            .header("Accept", "application/json, text/event-stream")
            .header("MCP-Protocol-Version", "2026-07-28")
            .header("Mcp-Method", "tools/call")
            .header("Mcp-Name", "host.quickstart")
            .header("Authorization", format!("Bearer {key}"))
            .header("User-Agent", ua)
            .json(&json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call",
                "params": {"name": "host.quickstart", "arguments": {"kind": "echo"},
                    "_meta": {"io.modelcontextprotocol/protocolVersion": "2026-07-28",
                              "io.modelcontextprotocol/clientCapabilities": {}}}}))
            .send()
            .await
            .expect("send");
        let body = common::parse_response_body(resp).await;
        let qs = extract_structured(&body["result"]);
        assert_eq!(qs["client"]["detected"], expect_detected, "{body}");
        let stored = server.state.db.find_tenant_by_id(tenant.id).await.expect("lookup").expect("tenant");
        assert_eq!(stored.client_name.as_deref(), Some("windsurf"), "after {ua}");
    }
}
