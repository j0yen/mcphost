//! AC10 — Given a live-mode key and a webhook event with
//! `livemode: false`, When it is POSTed, Then the event is ledgered with
//! `event_type` suffixed `.mode_mismatch` and the tenant's plan is
//! unchanged.

mod common;
use common::{McpClient, TestServer, signup};
use mcphost::billing::{BillingConfig, FakeBillingClient, sign_for_test};
use serde_json::json;
use std::sync::Arc;

const WEBHOOK_SECRET: &str = "whsec_test_ac10";

fn billing_config() -> BillingConfig {
    BillingConfig {
        secret_key: Some("sk_live_ac10".to_string()), // a live-mode key
        webhook_secret: Some(WEBHOOK_SECRET.to_string()),
        price_pro: Some("price_pro_ac10".to_string()),
    }
}

#[tokio::test]
async fn a_test_mode_event_against_a_live_key_is_ledgered_as_mode_mismatch_and_not_applied() {
    let server = TestServer::start_with_billing(
        billing_config(),
        Arc::new(FakeBillingClient::new(mcphost::state::now_unix())),
    )
    .await;
    let (ns, key) = signup(&server.base_url, "Confused Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let payload = json!({
        "id": "evt_ac10_1",
        "type": "checkout.session.completed",
        "livemode": false,
        "data": {"object": {"client_reference_id": ns, "customer": "cus_ac10"}}
    });
    let body = serde_json::to_vec(&payload).unwrap();
    let sig = sign_for_test(&body, WEBHOOK_SECRET, mcphost::state::now_unix());
    let resp = reqwest::Client::new()
        .post(format!("{}/billing/webhook", server.base_url))
        .header("Stripe-Signature", sig)
        .body(body)
        .send()
        .await
        .expect("POST /billing/webhook");
    assert_eq!(resp.status(), 200);

    let status = client
        .tools_call("billing.status", json!({}))
        .await
        .expect("billing.status");
    assert_eq!(
        common::extract_structured(&status)["plan"],
        json!("free"),
        "a mismatched event must never change the tenant's plan"
    );

    // The mismatch branch never resolves a tenant (checked before any
    // event-type dispatch, requirement: "ledgered ... and NOT applied"),
    // so the row it writes carries no tenant at all.
    let events = server
        .state
        .db
        .list_billing_events(None, None, None)
        .await
        .expect("list_billing_events");
    assert_eq!(events.len(), 1, "{events:?}");
    assert_eq!(
        events[0].event_type,
        "checkout.session.completed.mode_mismatch"
    );
    assert_eq!(events[0].mode, "test");
    assert_eq!(events[0].tenant, None);
}
