//! PRD-mcphost-alerting-webhook
//! AC1 — Given `MCPHOST_ALERT_WEBHOOK_URL` points at a test server, When
//! signups are paused via the pause file, Then within 30 s the server
//! receives one POST with `key: "signup.paused"` and the `alerts` row has
//! `delivery_status: "delivered"`.

use crate::common;
use common::TestServer;
use serde_json::Value;
use std::time::Duration;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn signup_pause_raises_and_delivers_a_webhook_alert() {
    let mock = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/alerts"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&mock)
        .await;

    let alert_config = mcphost::alerts::AlertConfig {
        webhook_url: Some(format!("{}/alerts", mock.uri())),
        ..mcphost::alerts::AlertConfig::default()
    };
    let server = TestServer::start_with_alert_config(alert_config).await;

    std::fs::write(server.state.signup_pause.path(), "maintenance window\n")
        .expect("write pause file");

    // Poll for up to 30s (AC1's own bound) for the pause-watch tick to
    // detect the transition, raise the alert, and its background delivery
    // task to finish.
    let mut delivered_row = None;
    for _ in 0..60 {
        if let Ok(Some(row)) = server
            .state
            .db
            .most_recent_alert_for_key("signup.paused".to_string())
            .await
            && row.delivery_status == "delivered"
        {
            delivered_row = Some(row);
            break;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }

    let row = delivered_row.expect("a signup.paused alert reached delivery_status: delivered within 30s");
    assert_eq!(row.delivery_status, "delivered");

    let requests = mock.received_requests().await.expect("mock records requests");
    assert_eq!(requests.len(), 1, "exactly one POST expected");
    let body: Value = requests[0].body_json().expect("valid json body");
    assert_eq!(body["key"], "signup.paused");
    assert_eq!(body["id"], row.id);
}
