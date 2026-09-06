//! AC8 — Given metering configured and 30 unemitted pro ok calls, When
//! `/healthz` is read, Then `meter_lag` is 30; and Given metering
//! unconfigured, Then the key is absent.

mod common;
use common::{TestServer, record_ok_calls, signup_and_make_pro};
use mcphost::billing::{BillingConfig, FakeBillingClient};
use std::sync::Arc;

#[tokio::test]
async fn meter_lag_present_and_accurate_when_configured() {
    let billing_config = BillingConfig {
        secret_key: Some("sk_test_metering_ac8".to_string()),
        webhook_secret: Some("whsec_test_metering_ac8".to_string()),
        price_pro: Some("price_pro_metering_ac8".to_string()),
        metered_price_id: Some("price_metered_ac8".to_string()),
        meter_event_name: None,
    };
    let server = TestServer::start_with_billing(
        billing_config,
        Arc::new(FakeBillingClient::new(mcphost::state::now_unix())),
    )
    .await;
    let (_ns, _key, pro_id) =
        signup_and_make_pro(&server, "Lag Tenant", "cus_metering_ac8").await;
    record_ok_calls(&server, pro_id, "some_tool", 30).await;

    let resp: serde_json::Value = reqwest::get(format!("{}/healthz", server.base_url))
        .await
        .expect("GET /healthz")
        .json()
        .await
        .expect("parse healthz body");
    assert_eq!(resp["meter_lag"], serde_json::json!(30));
}

#[tokio::test]
async fn meter_lag_absent_when_metering_unconfigured() {
    let server = TestServer::start().await;

    let resp: serde_json::Value = reqwest::get(format!("{}/healthz", server.base_url))
        .await
        .expect("GET /healthz")
        .json()
        .await
        .expect("parse healthz body");
    assert!(
        resp.get("meter_lag").is_none(),
        "meter_lag must be entirely absent, not present-and-zero: {resp:?}"
    );
}
