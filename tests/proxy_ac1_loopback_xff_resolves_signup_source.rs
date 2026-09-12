//! PRD-mcphost-client-ip-behind-proxy
//! AC1 — Given a request whose peer is `127.0.0.1` and whose
//! `X-Forwarded-For` is `203.0.113.7, 10.0.0.1`, When the handler resolves
//! the source, Then the source is `203.0.113.7`.
//!
//! `TestServer` binds to `127.0.0.1`, so every real request in this suite
//! has a genuinely loopback TCP peer -- no mocking needed to exercise the
//! "peer is loopback" branch of `state::resolve_source_ip`. This test
//! drives the resolution end to end through the real `signup` call and
//! reads the result back out of `signup_events` (via
//! `Db::signup_count_since`, the same query the rate limiter itself uses)
//! rather than asserting on an internal function directly.

use crate::common;
use common::{McpClient, TestServer};
use serde_json::json;

#[tokio::test]
async fn loopback_peer_with_xff_resolves_to_first_forwarded_address() {
    let server = TestServer::start().await;
    let client = McpClient::new(&server.base_url);

    client
        .tools_call_with_header(
            "signup",
            json!({"name": "Forwarded Agent"}),
            ("x-forwarded-for", "203.0.113.7, 10.0.0.1"),
        )
        .await
        .unwrap_or_else(|e| panic!("signup should succeed: {e:?}"));

    let since = mcphost::state::now_unix() - 60;
    let forwarded_count = server
        .state
        .db
        .signup_count_since("203.0.113.7".to_string(), since)
        .await
        .expect("query signup_events");
    assert_eq!(
        forwarded_count, 1,
        "the resolved source must be the first X-Forwarded-For address, not the second"
    );

    let peer_count = server
        .state
        .db
        .signup_count_since("127.0.0.1".to_string(), since)
        .await
        .expect("query signup_events");
    assert_eq!(
        peer_count, 0,
        "the raw loopback peer must not be the address stored once a header resolves it"
    );
}
