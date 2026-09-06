//! AC8 — Given a `pro` tenant, When a signed
//! `customer.subscription.deleted` arrives, Then the tenant's `plan` is
//! `free`, a row is ledgered, and `billing.status` reports `plan: free`.

mod common;
use common::{McpClient, TestServer, signup};
use mcphost::billing::{BillingConfig, FakeBillingClient, sign_for_test};
use serde_json::json;
use std::sync::Arc;

const WEBHOOK_SECRET: &str = "whsec_test_ac8";

fn billing_config() -> BillingConfig {
    BillingConfig {
        secret_key: Some("sk_test_ac8".to_string()),
        webhook_secret: Some(WEBHOOK_SECRET.to_string()),
        price_pro: Some("price_pro_ac8".to_string()),
    }
}

async fn post_signed(server: &common::TestServer, payload: &serde_json::Value) -> reqwest::StatusCode {
    let body = serde_json::to_vec(payload).unwrap();
    let sig = sign_for_test(&body, WEBHOOK_SECRET, mcphost::state::now_unix());
    reqwest::Client::new()
        .post(format!("{}/billing/webhook", server.base_url))
        .header("Stripe-Signature", sig)
        .body(body)
        .send()
        .await
        .expect("POST /billing/webhook")
        .status()
}

#[tokio::test]
async fn subscription_deleted_downgrades_the_tenant_to_free() {
    let server = TestServer::start_with_billing(
        billing_config(),
        Arc::new(FakeBillingClient::new(mcphost::state::now_unix())),
    )
    .await;
    let (ns, key) = signup(&server.base_url, "Lapsing Pro").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    // Get the tenant to `pro` (with a `billing_ref` the deletion event
    // below can look it back up by) via a real `checkout.session.completed`
    // -- AC8 only cares about the downgrade path, but it has to start from
    // a tenant that actually has a `billing_ref` set, same as production.
    let setup = json!({
        "id": "evt_ac8_setup_checkout",
        "type": "checkout.session.completed",
        "livemode": false,
        "data": {"object": {"client_reference_id": ns, "customer": "cus_ac8"}}
    });
    assert_eq!(post_signed(&server, &setup).await, 200);

    let status = client
        .tools_call("billing.status", json!({}))
        .await
        .expect("billing.status pre-downgrade");
    assert_eq!(common::extract_structured(&status)["plan"], json!("pro"));

    let delete = json!({
        "id": "evt_ac8_subscription_deleted",
        "type": "customer.subscription.deleted",
        "livemode": false,
        "data": {"object": {"customer": "cus_ac8"}}
    });
    assert_eq!(post_signed(&server, &delete).await, 200);

    let status_after = client
        .tools_call("billing.status", json!({}))
        .await
        .expect("billing.status after downgrade");
    assert_eq!(
        common::extract_structured(&status_after)["plan"],
        json!("free")
    );

    let events = server
        .state
        .db
        .list_billing_events(None, None, Some(ns))
        .await
        .expect("list_billing_events");
    assert!(
        events
            .iter()
            .any(|e| e.event_type == "customer.subscription.deleted"),
        "{events:?}"
    );
}
