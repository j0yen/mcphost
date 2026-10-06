//! PRD-mcphost-upgrade-moment AC5 — Given a tenant with one completed
//! checkout, When `billing.status` is called, Then `receipt` carries
//! plan, amount_cents, currency, paid_at, invoice_ref and mode; given
//! none, Then `receipt` is absent.

use crate::common;
use common::{McpClient, TestServer, signup};
use mcphost::billing::{BillingConfig, FakeBillingClient, sign_for_test};
use serde_json::json;
use std::sync::Arc;

const WEBHOOK_SECRET: &str = "whsec_test_upgrade_ac5";

fn billing_config() -> BillingConfig {
    BillingConfig {
        secret_key: Some("sk_test_upgrade_ac5".to_string()),
        webhook_secret: Some(WEBHOOK_SECRET.to_string()),
        price_pro: Some("price_pro_upgrade_ac5".to_string()),
        metered_price_id: None,
        meter_event_name: None,
    }
}

#[tokio::test]
async fn status_has_no_receipt_before_any_checkout_completes() {
    let server = TestServer::start_with_billing(
        billing_config(),
        Arc::new(FakeBillingClient::new(mcphost::state::now_unix())),
    )
    .await;
    let (_ns, key) = signup(&server.base_url, "No Receipt Yet").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let status = client.tools_call("billing.status", json!({})).await.expect("billing.status");
    let structured = common::extract_structured(&status);
    assert!(structured.get("receipt").is_none(), "{structured:?}");
}

#[tokio::test]
async fn status_carries_a_receipt_after_a_completed_checkout() {
    let server = TestServer::start_with_billing(
        billing_config(),
        Arc::new(FakeBillingClient::new(mcphost::state::now_unix())),
    )
    .await;
    let (ns, key) = signup(&server.base_url, "Freshly Paid").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let payload = json!({
        "id": "evt_ac5_checkout_completed",
        "type": "checkout.session.completed",
        "livemode": false,
        "data": {
            "object": {
                "id": "cs_ac5",
                "client_reference_id": ns,
                "customer": "cus_ac5",
                "subscription": "sub_ac5",
                "amount_total": 1900,
                "currency": "usd",
                "metadata": {"source": "tools_max"},
            }
        }
    });
    let body = serde_json::to_vec(&payload).unwrap();
    let signature = sign_for_test(&body, WEBHOOK_SECRET, mcphost::state::now_unix());
    let resp = reqwest::Client::new()
        .post(format!("{}/billing/webhook", server.base_url))
        .header("Stripe-Signature", &signature)
        .header("Content-Type", "application/json")
        .body(body)
        .send()
        .await
        .expect("POST /billing/webhook");
    assert_eq!(resp.status(), 200);

    let status = client.tools_call("billing.status", json!({})).await.expect("billing.status");
    let structured = common::extract_structured(&status);
    let receipt = &structured["receipt"];
    assert_eq!(receipt["plan"], json!("pro"), "{receipt:?}");
    assert_eq!(receipt["amount_cents"], json!(1900), "{receipt:?}");
    assert_eq!(receipt["currency"], json!("usd"), "{receipt:?}");
    assert_eq!(receipt["mode"], json!("test"), "{receipt:?}");
    assert!(receipt["paid_at"].is_string(), "{receipt:?}");
    // `invoice_ref` is the tenant's own `billing_ref` -- `customer` wins
    // over the `subscription` fallback when both are present on the
    // webhook event (same priority `apply_checkout_completed` already
    // gives `billing_ref`).
    assert_eq!(receipt["invoice_ref"], json!("cus_ac5"), "{receipt:?}");
}
