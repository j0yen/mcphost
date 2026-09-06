//! AC2 — Given a signed `checkout.session.completed` with a `customer` id,
//! When the webhook processes it, Then the tenant row stores that id (as
//! `stripe_customer_id`, distinct from the pre-existing `billing_ref`) and
//! the upgrade behavior of grand-loop-billing's AC6 is unchanged.

mod common;
use common::{McpClient, TestServer, signup};
use mcphost::billing::{BillingConfig, FakeBillingClient, sign_for_test};
use serde_json::json;
use std::sync::Arc;

const WEBHOOK_SECRET: &str = "whsec_test_metering_ac2";

fn billing_config() -> BillingConfig {
    BillingConfig {
        secret_key: Some("sk_test_metering_ac2".to_string()),
        webhook_secret: Some(WEBHOOK_SECRET.to_string()),
        price_pro: Some("price_pro_metering_ac2".to_string()),
        metered_price_id: None,
        meter_event_name: None,
    }
}

#[tokio::test]
async fn checkout_completed_stores_the_stripe_customer_id() {
    let server = TestServer::start_with_billing(
        billing_config(),
        Arc::new(FakeBillingClient::new(mcphost::state::now_unix())),
    )
    .await;
    let (ns, key) = signup(&server.base_url, "Soon Pro").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let tenant_before = server
        .state
        .db
        .find_tenant_by_namespace(ns.clone())
        .await
        .expect("find tenant")
        .expect("tenant exists");
    assert_eq!(tenant_before.stripe_customer_id, None);

    let payload = json!({
        "id": "evt_metering_ac2_checkout_completed",
        "type": "checkout.session.completed",
        "livemode": false,
        "data": {
            "object": {
                "client_reference_id": ns,
                "customer": "cus_metering_ac2",
                "subscription": "sub_metering_ac2",
                "amount_total": 1900,
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
        .body(body)
        .send()
        .await
        .expect("POST /billing/webhook");
    assert_eq!(resp.status(), 200);

    // grand-loop-billing's AC6 behavior: the tenant is upgraded to pro.
    let status = client
        .tools_call("billing.status", json!({}))
        .await
        .expect("billing.status after webhook");
    assert_eq!(common::extract_structured(&status)["plan"], json!("pro"));

    // This PRD's new column: the customer id specifically, not the
    // subscription id `billing_ref` would also have accepted.
    let tenant_after = server
        .state
        .db
        .find_tenant_by_namespace(ns)
        .await
        .expect("find tenant")
        .expect("tenant exists");
    assert_eq!(
        tenant_after.stripe_customer_id.as_deref(),
        Some("cus_metering_ac2")
    );
    assert_eq!(tenant_after.billing_ref.as_deref(), Some("cus_metering_ac2"));
}
