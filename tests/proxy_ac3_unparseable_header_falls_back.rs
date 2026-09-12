//! PRD-mcphost-client-ip-behind-proxy
//! AC3 — Given a request whose peer is loopback and whose header is
//! `not-an-ip`, When the handler resolves the source, Then the source is
//! the peer address and the request is not rejected.

use crate::common;
use common::{McpClient, TestServer};
use serde_json::json;

#[test]
fn unparseable_header_resolves_to_peer_directly() {
    let resolved = mcphost::state::resolve_source_ip(Some("127.0.0.1"), Some("not-an-ip"));
    assert_eq!(resolved, "127.0.0.1");
}

/// End-to-end: an unparseable `X-Forwarded-For` from a real (loopback)
/// peer must not reject the request -- `signup` still succeeds, and the
/// stored `signup_events.source_ip` is the peer address.
#[tokio::test]
async fn unparseable_header_does_not_reject_signup() {
    let server = TestServer::start().await;
    let client = McpClient::new(&server.base_url);

    client
        .tools_call_with_header(
            "signup",
            json!({"name": "Bad Header Agent"}),
            ("x-forwarded-for", "not-an-ip"),
        )
        .await
        .unwrap_or_else(|e| {
            panic!("an unparseable X-Forwarded-For must not reject the signup: {e:?}")
        });

    let since = mcphost::state::now_unix() - 60;
    let peer_count = server
        .state
        .db
        .signup_count_since("127.0.0.1".to_string(), since)
        .await
        .expect("query signup_events");
    assert_eq!(
        peer_count, 1,
        "an unparseable header must fall back to the peer address"
    );
}
