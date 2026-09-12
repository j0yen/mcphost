//! PRD-mcphost-client-ip-behind-proxy
//! AC5 — Given a signup proxied from loopback with a forwarded address,
//! When it completes, Then the `signup_events` row for it stores the
//! forwarded address.

use crate::common;
use common::{McpClient, TestServer};
use serde_json::json;

#[tokio::test]
async fn signup_event_row_stores_the_forwarded_address() {
    let server = TestServer::start().await;
    let client = McpClient::new(&server.base_url);

    client
        .tools_call_with_header(
            "signup",
            json!({"name": "Attributed Agent"}),
            ("x-forwarded-for", "203.0.113.9"),
        )
        .await
        .unwrap_or_else(|e| panic!("signup should succeed: {e:?}"));

    let since = mcphost::state::now_unix() - 60;
    let stored = server
        .state
        .db
        .signup_count_since("203.0.113.9".to_string(), since)
        .await
        .expect("query signup_events");
    assert_eq!(
        stored, 1,
        "signup_events.source_ip must store the forwarded address, not 127.0.0.1"
    );
}
