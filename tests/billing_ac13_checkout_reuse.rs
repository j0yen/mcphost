//! AC13 (P1) — Given an open unexpired checkout for a tenant, When it
//! calls `billing.checkout` again for the same plan, Then the same `url`
//! is returned and the fake client received no second create.

mod common;
use common::{McpClient, TestServer, signup};
use mcphost::billing::{BillingConfig, FakeBillingClient};
use serde_json::json;
use std::sync::Arc;

fn test_billing_config() -> BillingConfig {
    BillingConfig {
        secret_key: Some("sk_test_reuse".to_string()),
        webhook_secret: Some("whsec_reuse".to_string()),
        price_pro: Some("price_pro_test".to_string()),
        metered_price_id: None,
        meter_event_name: None,
    }
}

#[tokio::test]
async fn second_checkout_for_same_tenant_and_plan_reuses_the_open_session() {
    let fake = Arc::new(FakeBillingClient::new(mcphost::state::now_unix()));
    let server = TestServer::start_with_billing(test_billing_config(), fake.clone()).await;
    let (_ns, key) = signup(&server.base_url, "Reuse Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let first = client
        .tools_call("billing.checkout", json!({"plan": "pro"}))
        .await
        .expect("first checkout must succeed");
    let first_structured = common::extract_structured(&first);

    let second = client
        .tools_call("billing.checkout", json!({"plan": "pro"}))
        .await
        .expect("second checkout must succeed");
    let second_structured = common::extract_structured(&second);

    assert_eq!(
        first_structured["url"], second_structured["url"],
        "a second call before expiry must return the same URL"
    );
    assert_eq!(
        first_structured["expires_at"],
        second_structured["expires_at"]
    );
    assert_eq!(
        fake.call_count(),
        1,
        "the fake billing client must not have been asked to create a second session"
    );
}

#[tokio::test]
async fn different_tenants_each_get_their_own_session() {
    let fake = Arc::new(FakeBillingClient::new(mcphost::state::now_unix()));
    let server = TestServer::start_with_billing(test_billing_config(), fake.clone()).await;

    let (_ns_a, key_a) = signup(&server.base_url, "Tenant A").await;
    let (_ns_b, key_b) = signup(&server.base_url, "Tenant B").await;
    let client_a = McpClient::with_bearer(&server.base_url, &key_a);
    let client_b = McpClient::with_bearer(&server.base_url, &key_b);

    let a = client_a
        .tools_call("billing.checkout", json!({"plan": "pro"}))
        .await
        .expect("tenant A checkout must succeed");
    let b = client_b
        .tools_call("billing.checkout", json!({"plan": "pro"}))
        .await
        .expect("tenant B checkout must succeed");

    let a_structured = common::extract_structured(&a);
    let b_structured = common::extract_structured(&b);

    assert_ne!(
        a_structured["url"], b_structured["url"],
        "two different tenants must not share a cached session"
    );
    assert_eq!(fake.call_count(), 2, "one create call per distinct tenant");
}
