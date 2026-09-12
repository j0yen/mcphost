//! AC3 — Given `metered_price_id` configured and a fake billing client,
//! When `billing.checkout` runs, Then the fake receives two line items
//! (base with quantity 1, metered without quantity) plus
//! `automatic_tax: true`; and Given no `metered_price_id`, Then it
//! receives exactly the single-price call shape from v0.14.0.

use crate::common;
use common::{McpClient, TestServer, signup};
use mcphost::billing::{BillingConfig, FakeBillingClient};
use serde_json::json;
use std::sync::Arc;

#[tokio::test]
async fn metered_price_id_configured_adds_a_second_line_item() {
    let now = mcphost::state::now_unix();
    let fake = Arc::new(FakeBillingClient::new(now));
    let billing_config = BillingConfig {
        secret_key: Some("sk_test_metering_ac3".to_string()),
        webhook_secret: Some("whsec_test_metering_ac3".to_string()),
        price_pro: Some("price_pro_metering_ac3".to_string()),
        metered_price_id: Some("price_metered_ac3".to_string()),
        meter_event_name: None,
    };
    let server = TestServer::start_with_billing(billing_config, fake.clone()).await;
    let (_ns, key) = signup(&server.base_url, "Metered Upgrader").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call("billing.checkout", json!({}))
        .await
        .expect("checkout must succeed");

    let last = fake
        .last_request
        .lock()
        .unwrap()
        .clone()
        .expect("fake received a request");
    assert_eq!(last.price_id, "price_pro_metering_ac3");
    assert_eq!(last.metered_price_id.as_deref(), Some("price_metered_ac3"));
    assert!(last.automatic_tax, "automatic_tax must be true when billing is configured");
}

#[tokio::test]
async fn no_metered_price_id_keeps_the_single_price_shape() {
    let now = mcphost::state::now_unix();
    let fake = Arc::new(FakeBillingClient::new(now));
    let billing_config = BillingConfig {
        secret_key: Some("sk_test_metering_ac3b".to_string()),
        webhook_secret: Some("whsec_test_metering_ac3b".to_string()),
        price_pro: Some("price_pro_metering_ac3b".to_string()),
        metered_price_id: None,
        meter_event_name: None,
    };
    let server = TestServer::start_with_billing(billing_config, fake.clone()).await;
    let (_ns, key) = signup(&server.base_url, "Unmetered Upgrader").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call("billing.checkout", json!({}))
        .await
        .expect("checkout must succeed");

    let last = fake
        .last_request
        .lock()
        .unwrap()
        .clone()
        .expect("fake received a request");
    assert_eq!(last.price_id, "price_pro_metering_ac3b");
    assert_eq!(
        last.metered_price_id, None,
        "v0.14.0's single-price shape must be unchanged when metering is not configured"
    );
    // AC3's automatic_tax flag is unconditional on billing being
    // configured at all, independent of the metered-price flag.
    assert!(last.automatic_tax);
}
