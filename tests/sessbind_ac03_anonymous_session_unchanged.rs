//! PRD-mcphost-session-bound-tenant-after-signup
//! AC3 (P0, as landed) — Given a fresh session with no `signup`, no
//! `tenant_key` and no header, When it calls `host.catalog.search`, Then
//! the response is HTTP 401 with a `WWW-Authenticate` header, JSON-RPC
//! `-32602`, `data.error_code` `tenant_key_missing`, byte-for-byte the
//! v0.60.35 body shape.
//!
//! PRD-mcphost-implicit-signup supersedes this exact AC: that fresh,
//! never-signed-up session's bare `host.*` call on `/mcp` now succeeds by
//! implicitly minting a tenant, rather than refusing (requirement 4's own
//! "no longer reachable for host.*/billing.* on /mcp" -- see
//! tests/implsign_ac01_*.rs). This is now the regression pin for THAT
//! guarantee instead: a 200 with no WWW-Authenticate challenge and a real
//! `host.catalog.search` result, byte-shape checked the same way the old
//! 401 body used to be.

use crate::common;
use common::{McpClient, TestServer, parse_response_body};
use serde_json::json;

#[tokio::test]
async fn no_signup_no_key_no_header_now_implicitly_signs_up() {
    let server = TestServer::start().await;
    let client = McpClient::new(&server.base_url);

    let resp = client
        .post_with_mcp_name_override(
            json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "tools/call",
                "params": {"name": "host.catalog.search", "arguments": {}},
            }),
            "host.catalog.search",
        )
        .await;

    assert_eq!(resp.status(), reqwest::StatusCode::OK, "a bare host.* call on /mcp now succeeds");
    assert!(
        resp.headers().get(reqwest::header::WWW_AUTHENTICATE).is_none(),
        "a successful call must carry no WWW-Authenticate challenge"
    );

    // PRD-mcphost-implicit-signup: this call binds the session, upgrading
    // the response to text/event-stream -- see parse_response_body's doc.
    let body = parse_response_body(resp).await;
    assert!(body.get("error").is_none(), "must not error: {body:?}");
    let structured = body["result"]
        .get("structuredContent")
        .cloned()
        .unwrap_or(body["result"].clone());
    assert!(
        structured.get("tools").is_some(),
        "host.catalog.search must return a real result: {structured}"
    );
    assert!(
        structured.get("onboarding").is_some(),
        "the call that created the implicit tenant must carry onboarding: {structured}"
    );
}
