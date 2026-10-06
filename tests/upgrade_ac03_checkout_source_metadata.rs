//! PRD-mcphost-upgrade-moment AC3 — Given
//! `billing.checkout(plan: "pro", source: "calls_per_day")`, When the
//! Stripe session is created (test mode), Then its metadata carries
//! `source = calls_per_day` and the response echoes `source`; an unknown
//! `source` is rejected with a validation error.

use crate::common;
use common::{McpClient, TestServer, signup};
use mcphost::billing::{BillingConfig, FakeBillingClient};
use serde_json::json;
use std::sync::Arc;

fn billing_config() -> BillingConfig {
    BillingConfig {
        secret_key: Some("sk_test_upgrade_ac3".to_string()),
        webhook_secret: Some("whsec_test_upgrade_ac3".to_string()),
        price_pro: Some("price_pro_upgrade_ac3".to_string()),
        metered_price_id: None,
        meter_event_name: None,
    }
}

#[tokio::test]
async fn checkout_with_a_known_source_carries_it_on_metadata_and_echoes_it() {
    let fake = Arc::new(FakeBillingClient::new(mcphost::state::now_unix()));
    let server = TestServer::start_with_billing(billing_config(), fake.clone()).await;
    let (_ns, key) = signup(&server.base_url, "Upgrader").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let result = client
        .tools_call(
            "billing.checkout",
            json!({"plan": "pro", "source": "calls_per_day"}),
        )
        .await
        .expect("checkout with a known source must succeed");
    let structured = common::extract_structured(&result);
    assert_eq!(structured["source"], json!("calls_per_day"), "{structured:?}");

    let last = fake
        .last_request
        .lock()
        .unwrap()
        .clone()
        .expect("fake received a request");
    assert_eq!(last.source, "calls_per_day");
}

#[tokio::test]
async fn checkout_with_no_source_defaults_to_manual() {
    let fake = Arc::new(FakeBillingClient::new(mcphost::state::now_unix()));
    let server = TestServer::start_with_billing(billing_config(), fake.clone()).await;
    let (_ns, key) = signup(&server.base_url, "Upgrader").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let result = client
        .tools_call("billing.checkout", json!({"plan": "pro"}))
        .await
        .expect("checkout with no source must succeed");
    let structured = common::extract_structured(&result);
    assert_eq!(structured["source"], json!("manual"), "{structured:?}");

    let last = fake.last_request.lock().unwrap().clone().expect("fake received a request");
    assert_eq!(last.source, "manual");
}

#[tokio::test]
async fn checkout_with_an_unknown_source_is_rejected() {
    let fake = Arc::new(FakeBillingClient::new(mcphost::state::now_unix()));
    let server = TestServer::start_with_billing(billing_config(), fake.clone()).await;
    let (_ns, key) = signup(&server.base_url, "Upgrader").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let err = client
        .tools_call(
            "billing.checkout",
            json!({"plan": "pro", "source": "not_a_real_refusal"}),
        )
        .await
        .expect_err("an unknown source must be rejected");
    assert_eq!(err.error_code.as_deref(), Some("invalid_params"));
    assert_eq!(fake.call_count(), 0, "no session must be created for a rejected source");
}
