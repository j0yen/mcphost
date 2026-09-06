//! AC5 — Given a fake billing client and a test-mode key, When a tenant
//! calls `billing.checkout`, Then the result has a `url`, an `expires_at`
//! about 24h ahead, `mode: test`, and `instructions` text, and the fake
//! received the pro price id and `client_reference_id` equal to the
//! tenant namespace.

mod common;
use common::{McpClient, TestServer, signup};
use mcphost::billing::{BillingConfig, FakeBillingClient};
use serde_json::json;
use std::sync::Arc;

#[tokio::test]
async fn checkout_with_a_test_key_returns_a_session_and_hits_the_fake_client() {
    let now = mcphost::state::now_unix();
    let fake = Arc::new(FakeBillingClient::new(now));
    let billing_config = BillingConfig {
        secret_key: Some("sk_test_ac5".to_string()),
        webhook_secret: Some("whsec_test_ac5".to_string()),
        price_pro: Some("price_pro_ac5".to_string()),
        metered_price_id: None,
        meter_event_name: None,
    };
    let server = TestServer::start_with_billing(billing_config, fake.clone()).await;
    let (ns, key) = signup(&server.base_url, "Upgrader").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let result = client
        .tools_call("billing.checkout", json!({}))
        .await
        .expect("checkout must succeed with a test key configured");
    let structured = common::extract_structured(&result);

    assert!(
        structured["url"]
            .as_str()
            .expect("url string")
            .starts_with("https://"),
        "{structured:?}"
    );
    assert_eq!(structured["mode"], json!("test"));
    assert!(
        structured["instructions"]
            .as_str()
            .expect("instructions string")
            .len()
            > 10
    );
    let expires_at = structured["expires_at"].as_i64().expect("expires_at");
    let delta = expires_at - now;
    assert!(
        (23 * 3600..=25 * 3600).contains(&delta),
        "expires_at should be about 24h ahead, got delta {delta}s"
    );

    assert_eq!(fake.call_count(), 1);
    let last = fake
        .last_request
        .lock()
        .unwrap()
        .clone()
        .expect("fake received a request");
    assert_eq!(last.price_id, "price_pro_ac5");
    assert_eq!(last.client_reference_id, ns);
    assert_eq!(last.tenant_namespace, ns);
}
