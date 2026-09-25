//! PRD-mcphost-alerting-webhook
//! AC8 — Given both sink env vars unset, When an alert is raised, Then it
//! is stored, nothing is sent, and healthz shows `alerts.sink:
//! "store-only"`.

use crate::common;
use common::{ADMIN_KEY, TestServer};
use serde_json::json;
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
async fn no_sinks_configured_stores_only() {
    // `AlertConfig::default()` -- neither webhook_url nor tenant set, same
    // "both sink env vars unset" precondition AC8 names.
    let server = TestServer::start_with_alert_config(mcphost::alerts::AlertConfig::default()).await;

    let hz_before = healthz(&server.base_url).await;
    assert_eq!(hz_before["alerts"]["sink"], "store-only");

    let alert_id = mcphost::alerts::raise(
        &server.state,
        mcphost::alerts::RaiseInput {
            key: "test.ac8.store_only".to_string(),
            severity: mcphost::alerts::Severity::Warn,
            title: "AC8 store-only".to_string(),
            body: json!({}),
        },
    )
    .await
    .expect("raise");

    let mut row = None;
    for _ in 0..30 {
        if let Ok(Some(r)) = server.state.db.get_alert_for_test(alert_id).await
            && r.delivery_status != "pending"
        {
            row = Some(r);
            break;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    let row = row.expect("delivery task finished within the test's own bound");
    assert_eq!(row.delivery_status, "store-only");
    assert!(row.delivered_at.is_none(), "store-only never sets delivered_at");

    let hz_after = healthz(&server.base_url).await;
    assert_eq!(hz_after["alerts"]["sink"], "store-only");
    assert!(hz_after["alerts"]["open"].as_i64().unwrap() >= 1);
}
