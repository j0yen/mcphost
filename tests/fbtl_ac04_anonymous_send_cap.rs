//! PRD-mcphost-feedback-tool-and-ledger AC4 — Given an anonymous IP that has
//! sent 5 times in the hour, When it sends a sixth, Then the error code is
//! `rate_limited` with `retry_after_s >= 1`, and a fleet IP is not capped.

use crate::common;
use common::{McpClient, TestServer};
use serde_json::json;

const SENDS: i64 = 5;

#[tokio::test]
async fn sixth_anonymous_send_in_an_hour_is_rate_limited() {
    let server = TestServer::start().await;
    let client = McpClient::new(&server.base_url);

    for i in 0..SENDS {
        client
            .tools_call("host.feedback.send", json!({"kind": "bug", "text": format!("n{i}")}))
            .await
            .unwrap_or_else(|e| panic!("send {i} is inside the cap: {e:?}"));
    }
    let err = client
        .tools_call("host.feedback.send", json!({"kind": "bug", "text": "one too many"}))
        .await
        .expect_err("the sixth send within the hour must be refused");
    assert_eq!(err.error_code.as_deref(), Some("rate_limited"));
    assert!(
        err.data["retry_after_s"].as_i64().is_some_and(|n| n >= 1),
        "retry_after_s must be >= 1: {:?}",
        err.data
    );
}

#[tokio::test]
async fn a_fleet_ip_is_not_capped() {
    let server = TestServer::start_with_signup_rate_limit_and_fleet_ips(5, "127.0.0.1").await;
    let client = McpClient::new(&server.base_url);

    for i in 0..(SENDS + 3) {
        client
            .tools_call("host.feedback.send", json!({"kind": "praise", "text": format!("n{i}")}))
            .await
            .unwrap_or_else(|e| panic!("a fleet IP send {i} must not be capped: {e:?}"));
    }
}
