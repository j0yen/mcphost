//! PRD-mcphost-unimplemented-method-status
//! AC3 (P0) -- Given `refusal_status`, When it is called for 2026-07-28,
//! 2025-11-25 and 2025-06-18 each with and without `Mcp-Session-Id`, and for a
//! request with no `MCP-Protocol-Version` header, and the gate receives a
//! batch of two refused methods on 2026-07-28, Then both 2026-07-28 calls
//! return 404, each earlier revision returns 200 with and without a session,
//! the no-header call returns 200, and the batch gets one 404 carrying both
//! errors.

use crate::common;
use common::TestServer;
use mcphost::capabilities::refusal_status;
use serde_json::{Value, json};

/// The status the gate answers a `nonexistent/method` POST with, sent with the
/// given `MCP-Protocol-Version` (or none) and optional session id.
async fn gate_status(base_url: &str, version: Option<&str>, session: Option<&str>) -> u16 {
    let mut req = reqwest::Client::new()
        .post(format!("{base_url}/mcp"))
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .body(json!({"jsonrpc": "2.0", "id": 1, "method": "nonexistent/method", "params": {}}).to_string());
    if let Some(v) = version {
        req = req.header("MCP-Protocol-Version", v);
    }
    if let Some(id) = session {
        req = req.header("Mcp-Session-Id", id);
    }
    req.send().await.expect("POST /mcp").status().as_u16()
}

#[tokio::test]
async fn refst_ac03_refusal_status_is_404_only_from_2026_07_28() {
    // The rule is a pure function of the protocol version: a session id is not an input.
    for session in [None, Some("sess-1")] {
        assert_eq!(refusal_status(Some("2026-07-28")).as_u16(), 404, "2026-07-28 session={session:?}");
        assert_eq!(refusal_status(Some("2025-11-25")).as_u16(), 200, "2025-11-25 session={session:?}");
        assert_eq!(refusal_status(Some("2025-06-18")).as_u16(), 200, "2025-06-18 session={session:?}");
    }
    assert_eq!(refusal_status(None).as_u16(), 200, "no MCP-Protocol-Version header");

    // And the gate agrees on the wire, with and without a session.
    let server = TestServer::start().await;
    for session in [None, Some("sess-1")] {
        for (version, want) in [(Some("2026-07-28"), 404), (Some("2025-11-25"), 200), (Some("2025-06-18"), 200), (None, 200)] {
            let got = gate_status(&server.base_url, version, session).await;
            assert_eq!(got, want, "gate for version={version:?} session={session:?}");
        }
    }
}

#[tokio::test]
async fn refst_ac03_a_batch_of_two_refused_methods_gets_one_404_with_both_errors() {
    let server = TestServer::start().await;
    let batch = json!([
        {"jsonrpc": "2.0", "id": 1, "method": "roots/list", "params": {}},
        {"jsonrpc": "2.0", "id": 2, "method": "nonexistent/method", "params": {}},
    ]);
    let resp = reqwest::Client::new()
        .post(format!("{}/mcp", server.base_url))
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .header("MCP-Protocol-Version", "2026-07-28")
        .body(batch.to_string())
        .send()
        .await
        .expect("POST /mcp");
    assert_eq!(resp.status().as_u16(), 404);
    let body: Value = resp.json().await.expect("JSON reply");
    let items = body.as_array().unwrap_or_else(|| panic!("batch reply is an array: {body}"));
    assert_eq!(items.len(), 2, "both errors in one reply: {body}");
    for (item, id) in items.iter().zip([1, 2]) {
        assert_eq!(item["id"], id, "{body}");
        assert_eq!(item["error"]["code"], -32601, "{body}");
        assert!(item["error"]["data"]["code"].is_string(), "{body}");
    }
}
