//! AC4 — Given a listen with no Authorization header and a listen whose
//! Authorization key matches no tenant, When each is POSTed, Then the
//! header-less one gets the `complete` result and closes within 1 s, the
//! unknown-key one ends with `tenant_key_invalid`, and neither case changes
//! tenants, `implicit_signup_memory`, `tenant_key_arg_memory`, `PeerRecents` or
//! the signup limiter.

use crate::common;
use common::{ListenStream, TestServer, signup};
use serde_json::{Value, json};
use std::time::Duration;

/// Every message until the server ends the stream (or `within` runs out).
async fn until_closed(stream: &mut ListenStream, within: Duration) -> Vec<Value> {
    let deadline = tokio::time::Instant::now() + within;
    let mut messages = Vec::new();
    while let Some(msg) = stream.next_message(deadline.saturating_duration_since(tokio::time::Instant::now())).await {
        messages.push(msg);
    }
    messages
}

fn count(server: &TestServer, sql: &str) -> i64 {
    let conn = rusqlite::Connection::open(server.data_dir.0.join("mcphost.db")).expect("open raw db");
    conn.query_row(sql, [], |r| r.get(0)).expect("count")
}

/// Everything a listen must leave alone.
fn footprint(server: &TestServer) -> (i64, i64, i64, usize, usize, usize) {
    let s = &server.state;
    (
        count(server, "SELECT COUNT(*) FROM tenants"),
        count(server, "SELECT COALESCE(SUM(last_seen_unix), 0) FROM tenants"),
        count(server, "SELECT COUNT(*) FROM signup_events"),
        s.implicit_signup_memory.len(),
        s.tenant_key_arg_memory.len(),
        s.redeemed_peers.len(),
    )
}

#[tokio::test]
async fn sublis_ac04_keyless_listen_completes_and_unknown_key_is_invalid_with_no_side_effects() {
    let server = TestServer::start().await;
    let (_ns, _key) = signup(&server.base_url, "sublis4").await;
    let before = footprint(&server);

    // No Authorization header: acknowledged, then the graceful result, then closed.
    let mut keyless = ListenStream::open_with(&server.base_url, None, 31, json!({"toolsListChanged": true})).await;
    assert_eq!(keyless.status, 200);
    let messages = until_closed(&mut keyless, Duration::from_secs(1)).await;
    assert!(keyless.is_closed(), "a keyless listen closes within 1 s: {messages:?}");
    assert_eq!(messages.first().map(|m| m["method"].clone()), Some(json!("notifications/subscriptions/acknowledged")), "{messages:?}");
    let last = messages.last().expect("a final message");
    assert_eq!(last["id"], json!(31), "{last}");
    assert_eq!(last["result"]["resultType"], json!("complete"), "{last}");
    assert_eq!(last["result"]["_meta"]["io.modelcontextprotocol/subscriptionId"], json!(31), "{last}");

    // A key no tenant has: the listen request ends with tenant_key_invalid.
    let unknown = "0".repeat(64);
    let mut invalid = ListenStream::open_with(&server.base_url, Some(&unknown), 32, json!({"toolsListChanged": true})).await;
    assert_eq!(invalid.status, 200);
    let messages = until_closed(&mut invalid, Duration::from_secs(5)).await;
    assert!(invalid.is_closed(), "an unknown-key listen ends: {messages:?}");
    let last = messages.last().expect("a final message");
    assert_eq!(last["id"], json!(32), "{last}");
    assert_eq!(last["error"]["data"]["error_code"], json!("tenant_key_invalid"), "{last}");

    assert_eq!(footprint(&server), before, "neither listen creates a tenant or touches session memory, peers or the signup limiter");
    assert_eq!(server.state.listen_hub.entries(), 0, "neither listen registers with the hub");
}
