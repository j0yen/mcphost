//! AC4 — Given a `pro`-plan tenant with a live Stripe subscription, When it
//! calls `host.self_offboard`, Then the Stripe subscription is canceled
//! (verified here against `FakeBillingClient`'s own record of what it
//! canceled -- the trait boundary `BillingClient::cancel_active_subscriptions`
//! exists precisely so this path is testable without a real network round
//! trip to Stripe; production wiring is `StripeClient::cancel_active_subscriptions`,
//! `DELETE https://api.stripe.com/v1/subscriptions/{id}`) and no further
//! invoice is generated -- modeled here as "no further billing activity is
//! possible," since the offboarded tenant's key is refused before it could
//! ever trigger one (pinned by AC2/AC3's own tests).

use crate::common;
use common::{McpClient, TestServer, signup_and_make_pro};
use mcphost::billing::{BillingConfig, FakeBillingClient};
use serde_json::json;
use std::sync::Arc;

#[tokio::test]
async fn self_offboard_cancels_the_pro_tenants_active_stripe_subscription() {
    let now = mcphost::state::now_unix();
    let fake = Arc::new(FakeBillingClient::new(now));
    let billing_config = BillingConfig {
        secret_key: Some("sk_test_ac4".to_string()),
        webhook_secret: Some("whsec_test_ac4".to_string()),
        price_pro: Some("price_pro_ac4".to_string()),
        metered_price_id: None,
        meter_event_name: None,
    };
    let server = TestServer::start_with_billing(billing_config, fake.clone()).await;

    let stripe_customer_id = "cus_ac4_offboard";
    let (ns, key, _tenant_id) =
        signup_and_make_pro(&server, "Paying Leaver", stripe_customer_id).await;
    fake.set_active_subscription(stripe_customer_id, "sub_ac4_live");

    let client = McpClient::with_bearer(&server.base_url, &key);
    let result = client
        .tools_call("host.self_offboard", json!({}))
        .await
        .expect("self_offboard must succeed for a pro tenant with a live subscription");
    let structured = common::extract_structured(&result);

    assert_eq!(structured["tenant"], json!(ns));
    assert_eq!(structured["disabled"], json!(true));
    assert_eq!(
        structured["billing_canceled"],
        json!(["sub_ac4_live"]),
        "self_offboard's own response must name the canceled subscription"
    );

    // The real side effect: the fake billing client (standing in for
    // Stripe) recorded that this subscription was actually canceled, not
    // just that the local row was flipped.
    assert_eq!(fake.canceled_subscriptions(), vec!["sub_ac4_live".to_string()]);

    // No further invoice is possible: the tenant's key is dead, so nothing
    // can call billing.checkout / trigger further metering as this tenant
    // from here on (AC3's own test pins the exact error shape).
    let err = client
        .tools_call("billing.status", json!({}))
        .await
        .expect_err("billing.status must be refused for an offboarded tenant's key");
    assert_eq!(err.error_code.as_deref(), Some("tenant_disabled"));
}

#[tokio::test]
async fn self_offboard_on_a_pro_tenant_with_no_active_subscription_is_not_an_error() {
    // A pro tenant whose subscription already lapsed (or a pro row whose
    // stripe_customer_id predates a real subscription) must still offboard
    // cleanly -- cancel_active_subscriptions returning an empty list is not
    // an error, per that method's own doc comment.
    let now = mcphost::state::now_unix();
    let fake = Arc::new(FakeBillingClient::new(now));
    let billing_config = BillingConfig {
        secret_key: Some("sk_test_ac4b".to_string()),
        webhook_secret: Some("whsec_test_ac4b".to_string()),
        price_pro: Some("price_pro_ac4b".to_string()),
        metered_price_id: None,
        meter_event_name: None,
    };
    let server = TestServer::start_with_billing(billing_config, fake.clone()).await;

    let (_ns, key, _tenant_id) =
        signup_and_make_pro(&server, "Lapsed Pro", "cus_ac4b_no_sub").await;
    // Deliberately no `set_active_subscription` call.

    let client = McpClient::with_bearer(&server.base_url, &key);
    let result = client
        .tools_call("host.self_offboard", json!({}))
        .await
        .expect("self_offboard must succeed even with zero active subscriptions to cancel");
    let structured = common::extract_structured(&result);
    assert_eq!(structured["disabled"], json!(true));
    assert_eq!(structured["billing_canceled"], json!([]));
    assert!(fake.canceled_subscriptions().is_empty());
}
