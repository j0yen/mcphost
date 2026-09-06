//! AC7 — Given an event with a bad signature or a timestamp older than
//! 300 s, When it is POSTed, Then the response is 400 and no row is
//! written.

mod common;
use common::TestServer;
use mcphost::billing::{BillingConfig, FakeBillingClient, sign_for_test};
use serde_json::json;
use std::sync::Arc;

const WEBHOOK_SECRET: &str = "whsec_test_ac7";

fn billing_config() -> BillingConfig {
    BillingConfig {
        secret_key: Some("sk_test_ac7".to_string()),
        webhook_secret: Some(WEBHOOK_SECRET.to_string()),
        price_pro: Some("price_pro_ac7".to_string()),
    }
}

fn payload() -> Vec<u8> {
    serde_json::to_vec(&json!({
        "id": "evt_ac7_1",
        "type": "checkout.session.completed",
        "livemode": false,
        "data": {"object": {"client_reference_id": "nobody"}}
    }))
    .unwrap()
}

#[tokio::test]
async fn wrong_signature_is_rejected_with_400_and_no_row() {
    let server = TestServer::start_with_billing(
        billing_config(),
        Arc::new(FakeBillingClient::new(mcphost::state::now_unix())),
    )
    .await;
    let body = payload();
    let bad_signature = sign_for_test(&body, "not-the-real-secret", mcphost::state::now_unix());

    let http = reqwest::Client::new();
    let resp = http
        .post(format!("{}/billing/webhook", server.base_url))
        .header("Stripe-Signature", bad_signature)
        .body(body)
        .send()
        .await
        .expect("POST /billing/webhook");
    assert_eq!(resp.status(), 400);

    let events = server
        .state
        .db
        .list_billing_events(None, None, None)
        .await
        .expect("list_billing_events");
    assert!(events.is_empty(), "{events:?}");
}

#[tokio::test]
async fn stale_timestamp_is_rejected_with_400_and_no_row() {
    let server = TestServer::start_with_billing(
        billing_config(),
        Arc::new(FakeBillingClient::new(mcphost::state::now_unix())),
    )
    .await;
    let body = payload();
    let old_timestamp = mcphost::state::now_unix() - 301;
    let signature = sign_for_test(&body, WEBHOOK_SECRET, old_timestamp);

    let http = reqwest::Client::new();
    let resp = http
        .post(format!("{}/billing/webhook", server.base_url))
        .header("Stripe-Signature", signature)
        .body(body)
        .send()
        .await
        .expect("POST /billing/webhook");
    assert_eq!(resp.status(), 400);

    let events = server
        .state
        .db
        .list_billing_events(None, None, None)
        .await
        .expect("list_billing_events");
    assert!(events.is_empty(), "{events:?}");
}
