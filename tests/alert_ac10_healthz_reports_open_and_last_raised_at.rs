//! PRD-mcphost-alerting-webhook
//! AC10 — Given the host is running, When `GET /healthz` is called with
//! the operator header, Then the body includes `alerts.open` and
//! `alerts.last_raised_at`.

use crate::common;
use common::{ADMIN_KEY, TestServer};
use std::time::Duration;

async fn healthz(base_url: &str) -> serde_json::Value {
    reqwest::Client::new()
        .get(format!("{base_url}/healthz"))
        .bearer_auth(ADMIN_KEY)
        .send()
        .await
        .expect("GET /healthz")
        .json()
        .await
        .expect("parse /healthz")
}

#[tokio::test]
async fn healthz_carries_alerts_open_and_last_raised_at() {
    let server = TestServer::start().await;

    let before = healthz(&server.base_url).await;
    assert_eq!(before["alerts"]["open"], 0, "{before:?}");
    assert!(before["alerts"]["last_raised_at"].is_null(), "{before:?}");

    let raised_at_floor = mcphost::state::now_unix();
    mcphost::alerts::raise(
        &server.state,
        mcphost::alerts::RaiseInput {
            key: "test.ac10.healthz".to_string(),
            severity: mcphost::alerts::Severity::Warn,
            title: "AC10 healthz".to_string(),
            body: serde_json::json!({}),
        },
    )
    .await
    .expect("raise");

    // The insert itself is synchronous within `raise` (non-functional
    // requirement: storage is synchronous, only delivery backgrounds), so
    // no poll/sleep is needed before this next read -- kept anyway as a
    // small safety margin against scheduler jitter.
    tokio::time::sleep(Duration::from_millis(200)).await;

    let after = healthz(&server.base_url).await;
    assert_eq!(after["alerts"]["open"], 1, "{after:?}");
    let last_raised_at = after["alerts"]["last_raised_at"].as_i64().expect("last_raised_at is a number");
    assert!(last_raised_at >= raised_at_floor, "{after:?}");
}
