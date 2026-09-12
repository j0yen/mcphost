//! AC6 — Given a signed `checkout.session.completed` event for that
//! tenant, When it is POSTed to `/billing/webhook`, Then the response is
//! 200, the tenant's `plan` is `pro` with `plan_since` set, one
//! `billing_events` row exists with `mode: test`, and a second POST of the
//! same event changes nothing and returns 200.

use crate::common;
use common::{McpClient, TestServer, signup};
use mcphost::billing::{BillingConfig, FakeBillingClient, sign_for_test};
use serde_json::json;
use std::sync::Arc;

const WEBHOOK_SECRET: &str = "whsec_test_ac6";

fn billing_config() -> BillingConfig {
    BillingConfig {
        secret_key: Some("sk_test_ac6".to_string()),
        webhook_secret: Some(WEBHOOK_SECRET.to_string()),
        price_pro: Some("price_pro_ac6".to_string()),
        metered_price_id: None,
        meter_event_name: None,
    }
}

#[tokio::test]
async fn checkout_completed_upgrades_the_tenant_and_is_idempotent() {
    let server = TestServer::start_with_billing(
        billing_config(),
        Arc::new(FakeBillingClient::new(mcphost::state::now_unix())),
    )
    .await;
    let (ns, key) = signup(&server.base_url, "Soon Pro").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    // Sanity: starts on free.
    let status = client
        .tools_call("billing.status", json!({}))
        .await
        .expect("billing.status");
    assert_eq!(common::extract_structured(&status)["plan"], json!("free"));

    let payload = json!({
        "id": "evt_ac6_checkout_completed",
        "type": "checkout.session.completed",
        "livemode": false,
        "data": {
            "object": {
                "client_reference_id": ns,
                "customer": "cus_ac6",
                "subscription": "sub_ac6",
                "amount_total": 2900,
                "currency": "usd",
            }
        }
    });
    let body = serde_json::to_vec(&payload).unwrap();
    let now = mcphost::state::now_unix();
    let signature = sign_for_test(&body, WEBHOOK_SECRET, now);

    let http = reqwest::Client::new();
    let resp = http
        .post(format!("{}/billing/webhook", server.base_url))
        .header("Stripe-Signature", &signature)
        .header("Content-Type", "application/json")
        .body(body.clone())
        .send()
        .await
        .expect("POST /billing/webhook");
    assert_eq!(resp.status(), 200);

    let status = client
        .tools_call("billing.status", json!({}))
        .await
        .expect("billing.status after webhook");
    let structured = common::extract_structured(&status);
    assert_eq!(structured["plan"], json!("pro"));
    assert!(structured["plan_since"].is_string());

    let events = server
        .state
        .db
        .list_billing_events(None, None, Some(ns.clone()))
        .await
        .expect("list_billing_events");
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].mode, "test");
    assert_eq!(events[0].event_type, "checkout.session.completed");

    // Second delivery of the identical event: 200, nothing changes.
    let resp2 = http
        .post(format!("{}/billing/webhook", server.base_url))
        .header("Stripe-Signature", &signature)
        .header("Content-Type", "application/json")
        .body(body)
        .send()
        .await
        .expect("POST /billing/webhook (duplicate)");
    assert_eq!(resp2.status(), 200);

    let events_after = server
        .state
        .db
        .list_billing_events(None, None, Some(ns))
        .await
        .expect("list_billing_events after duplicate");
    assert_eq!(events_after.len(), 1, "duplicate delivery must not add a row");
}
