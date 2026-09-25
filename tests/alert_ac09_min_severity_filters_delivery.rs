//! PRD-mcphost-alerting-webhook
//! AC9 — Given `MCPHOST_ALERT_MIN_SEVERITY=critical`, When a `warn` alert
//! is raised, Then it is stored and not delivered; a `critical` one is
//! delivered.

use crate::common;
use common::TestServer;
use std::time::Duration;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn warn_is_skipped_critical_is_delivered() {
    let mock = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/alerts"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&mock)
        .await;

    let alert_config = mcphost::alerts::AlertConfig {
        webhook_url: Some(format!("{}/alerts", mock.uri())),
        min_severity: mcphost::alerts::Severity::Critical,
        ..mcphost::alerts::AlertConfig::default()
    };
    let server = TestServer::start_with_alert_config(alert_config).await;

    let warn_id = mcphost::alerts::raise(
        &server.state,
        mcphost::alerts::RaiseInput {
            key: "test.ac9.warn".to_string(),
            severity: mcphost::alerts::Severity::Warn,
            title: "AC9 warn".to_string(),
            body: serde_json::json!({}),
        },
    )
    .await
    .expect("raise warn");

    let critical_id = mcphost::alerts::raise(
        &server.state,
        mcphost::alerts::RaiseInput {
            key: "test.ac9.critical".to_string(),
            severity: mcphost::alerts::Severity::Critical,
            title: "AC9 critical".to_string(),
            body: serde_json::json!({}),
        },
    )
    .await
    .expect("raise critical");

    let mut warn_row = None;
    let mut critical_row = None;
    for _ in 0..30 {
        if warn_row.is_none()
            && let Ok(Some(r)) = server.state.db.get_alert_for_test(warn_id).await
            && r.delivery_status != "pending"
        {
            warn_row = Some(r);
        }
        if critical_row.is_none()
            && let Ok(Some(r)) = server.state.db.get_alert_for_test(critical_id).await
            && r.delivery_status != "pending"
        {
            critical_row = Some(r);
        }
        if warn_row.is_some() && critical_row.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }

    let warn_row = warn_row.expect("warn delivery task finished");
    assert_eq!(warn_row.delivery_status, "skipped");
    assert!(warn_row.delivered_at.is_none());

    let critical_row = critical_row.expect("critical delivery task finished");
    assert_eq!(critical_row.delivery_status, "delivered");
    assert!(critical_row.delivered_at.is_some());

    let requests = mock.received_requests().await.expect("mock records requests");
    assert_eq!(requests.len(), 1, "only the critical alert reaches the webhook");
    let body: serde_json::Value = requests[0].body_json().expect("valid json body");
    assert_eq!(body["key"], "test.ac9.critical");
}
