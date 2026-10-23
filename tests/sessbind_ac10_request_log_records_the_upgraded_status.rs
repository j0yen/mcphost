//! PRD-mcphost-session-bound-tenant-after-signup
//! AC10 (P1) — Given any `tools/call` that `oauth_401_upgrade` rewrites to
//! 401, When the request-log line for it is read, Then its `status` field
//! is `401`, and for an unmodified 200 response it is `200`.
//!
//! Before requirement 7's fix, `src/http.rs`'s two middleware layers ran in
//! an order where `protocol_version_and_log` saw the response BEFORE
//! `oauth_401_upgrade` rewrote its status -- a `tenant_key_missing` refusal
//! (JSON-RPC 200 on the wire at that point, later rewritten to HTTP 401)
//! was journaled as `status: 200`. Layer order is now reversed so the log
//! line records the status actually sent to the client.
//!
//! Captures through `common::tracecap` (one process-global subscriber per
//! test binary, installed once, events routed to the emitting thread's
//! capture), not a scoped `tracing::subscriber::set_default` +
//! `rebuild_interest_cache()`: in this consolidated binary a concurrent
//! test could leave the `"http request"` callsite cached as "never
//! interested" while the scoped dispatcher was being swapped, and the line
//! was then MISSING (`left: None, right: Some(401)`, CI run 39681889523) --
//! a missing event, so no wait or retry could recover it. Safe to key by
//! thread because `#[tokio::test]` defaults to the current-thread runtime,
//! so the whole request round trip below runs on this same thread.

use crate::common;
use common::TestServer;
use common::tracecap::Capture;

/// The `fields.status` an `/mcp` "http request" log line recorded for the
/// most recent matching call, read back out of `buf`.
fn last_mcp_status(buf: &Capture) -> Option<u64> {
    buf.json_lines()
        .into_iter()
        .filter(|v| v.get("fields").and_then(|f| f.get("path")).and_then(|p| p.as_str()) == Some("/mcp"))
        .filter_map(|v| v["fields"]["status"].as_u64())
        .next_back()
}

#[tokio::test]
async fn refused_call_logs_401_and_ok_call_logs_200() {
    let server = TestServer::start().await;
    let buf = common::tracecap::capture_json();

    let http = reqwest::Client::new();

    // A key-less, header-less call: refused tenant_key_missing at the
    // JSON-RPC level, then rewritten to a real HTTP 401 by
    // `oauth_401_upgrade`.
    //
    // PRD-mcphost-implicit-signup: a bare host.* call (like the
    // `host.catalog.search` this test used to drive) now implicitly signs
    // up instead of refusing (requirement 4), so `admin.tenants` -- which
    // never implicit-signs-up (Non-goal) -- is the conduit here instead.
    let refused = http
        .post(format!("{}/mcp", server.base_url))
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .header("MCP-Protocol-Version", "2026-07-28")
        .header("Mcp-Method", "tools/call")
        .header("Mcp-Name", "admin.tenants")
        .json(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": {
                "name": "admin.tenants",
                "arguments": {},
                "_meta": {
                    "io.modelcontextprotocol/protocolVersion": "2026-07-28",
                    "io.modelcontextprotocol/clientCapabilities": {},
                },
            },
        }))
        .send()
        .await
        .expect("send refused request");
    assert_eq!(refused.status(), reqwest::StatusCode::UNAUTHORIZED, "must be upgraded to a real 401");
    assert_eq!(
        last_mcp_status(&buf),
        Some(401),
        "the request-log line for the refused call must record 401, not the pre-upgrade 200"
    );

    // signup: unauthenticated, unaffected by oauth_401_upgrade, plain 200.
    let ok = http
        .post(format!("{}/mcp", server.base_url))
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .header("MCP-Protocol-Version", "2026-07-28")
        .header("Mcp-Method", "tools/call")
        .header("Mcp-Name", "signup")
        .json(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/call",
            "params": {
                "name": "signup",
                "arguments": {"name": "AC10 Tenant"},
                "_meta": {
                    "io.modelcontextprotocol/protocolVersion": "2026-07-28",
                    "io.modelcontextprotocol/clientCapabilities": {},
                },
            },
        }))
        .send()
        .await
        .expect("send signup request");
    assert_eq!(ok.status(), reqwest::StatusCode::OK);
    assert_eq!(
        last_mcp_status(&buf),
        Some(200),
        "the request-log line for an unmodified success must still record 200"
    );
}
