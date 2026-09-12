//! AC8 — Given metering configured and 30 unemitted pro ok calls, When
//! `/healthz` is read, Then `meter_lag` is 30; and Given metering
//! unconfigured, Then the key is absent.

use crate::common;
use common::{ADMIN_KEY, TestServer, record_ok_calls, signup_and_make_pro};
use mcphost::billing::{BillingConfig, FakeBillingClient};
use std::sync::Arc;

// PRD-mcphost-healthz-minimal: `meter_lag` only ever appeared on the full
// diagnostics document, which now requires the admin bearer -- the
// anonymous body is `{"ok": true/false}` and never carries it.
async fn admin_healthz(base_url: &str) -> serde_json::Value {
    reqwest::Client::new()
        .get(format!("{base_url}/healthz"))
        .bearer_auth(ADMIN_KEY)
        .send()
        .await
        .expect("GET /healthz")
        .json()
        .await
        .expect("parse healthz body")
}

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

    let resp = admin_healthz(&server.base_url).await;
    assert_eq!(resp["meter_lag"], serde_json::json!(30));
}

#[tokio::test]
async fn meter_lag_absent_when_metering_unconfigured() {
    let server = TestServer::start().await;

    let resp = admin_healthz(&server.base_url).await;
    assert!(
        resp.get("meter_lag").is_none(),
        "meter_lag must be entirely absent, not present-and-zero: {resp:?}"
    );
}
