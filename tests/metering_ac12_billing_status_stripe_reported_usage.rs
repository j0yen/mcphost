//! AC12 (P1) — Given a configured key and a pro tenant, When `billing.status`
//! runs with the fake returning an accepted-usage total, Then the response
//! includes that total labeled as Stripe-reported.

use crate::common;
use common::{McpClient, TestServer, record_ok_calls, signup, signup_and_make_pro};
use mcphost::billing::{BillingConfig, FakeBillingClient};
use serde_json::json;
use std::sync::Arc;

fn configured_billing() -> BillingConfig {
    BillingConfig {
        secret_key: Some("sk_test_ac12".to_string()),
        webhook_secret: Some("whsec_test_ac12".to_string()),
        price_pro: Some("price_pro_ac12".to_string()),
        metered_price_id: None,
        meter_event_name: None,
    }
}

#[tokio::test]
async fn pro_tenant_status_includes_stripe_reported_usage() {
    let now = mcphost::state::now_unix();
    let fake = Arc::new(FakeBillingClient::new(now));
    let server = TestServer::start_with_billing(configured_billing(), fake.clone()).await;
    let (_ns, key, tenant_id) =
        signup_and_make_pro(&server, "Stripe Reported", "cus_ac12_reported").await;
    record_ok_calls(&server, tenant_id, "some_tool", 4).await;
    fake.set_accepted_usage(123);

    let client = McpClient::with_bearer(&server.base_url, &key);
    let result = client
        .tools_call("billing.status", json!({}))
        .await
        .expect("billing.status");
    let status = common::extract_structured(&result);

    assert_eq!(status["plan"], json!("pro"));
    assert_eq!(
        status["metered_usage"]["stripe_reported"],
        json!(123),
        "{status:?}"
    );
    // The ledger's own count is unaffected -- these are two distinct
    // numbers, per requirement 60's "labeled as Stripe-reported" (not a
    // replacement for `emitted_this_month`).
    assert_eq!(status["metered_usage"]["emitted_this_month"], json!(0));
}

#[tokio::test]
async fn free_tenant_status_never_queries_stripe_reported_usage() {
    let now = mcphost::state::now_unix();
    let fake = Arc::new(FakeBillingClient::new(now));
    let server = TestServer::start_with_billing(configured_billing(), fake.clone()).await;
    let (_ns, key) = signup(&server.base_url, "Free Stays Free").await;
    fake.set_accepted_usage(999);

    let client = McpClient::with_bearer(&server.base_url, &key);
    let result = client
        .tools_call("billing.status", json!({}))
        .await
        .expect("billing.status");
    let status = common::extract_structured(&result);

    assert_eq!(status["plan"], json!("free"));
    assert!(
        status.get("metered_usage").is_none(),
        "a free tenant's status must not carry metered_usage, Stripe-reported or otherwise: {status:?}"
    );
}

#[tokio::test]
async fn pro_tenant_without_a_stripe_customer_id_gets_no_stripe_reported_field() {
    // A pro tenant promoted without ever completing a real Stripe checkout
    // (no `stripe_customer_id` on file) has nothing for `accepted_usage` to
    // query -- `billing.status` must not call the billing client at all,
    // let alone surface a bogus number.
    let now = mcphost::state::now_unix();
    let fake = Arc::new(FakeBillingClient::new(now));
    let server = TestServer::start_with_billing(configured_billing(), fake.clone()).await;
    let (ns, key) = signup(&server.base_url, "Pro No Customer Id").await;
    let tenant = server
        .state
        .db
        .find_tenant_by_namespace(ns)
        .await
        .expect("find tenant")
        .expect("tenant exists");
    server
        .state
        .db
        .upgrade_tenant_plan(tenant.id, "pro".to_string(), mcphost::state::rfc3339_now(), None)
        .await
        .expect("upgrade to pro");
    fake.set_accepted_usage(555);

    let client = McpClient::with_bearer(&server.base_url, &key);
    let result = client
        .tools_call("billing.status", json!({}))
        .await
        .expect("billing.status");
    let status = common::extract_structured(&result);

    assert_eq!(status["plan"], json!("pro"));
    assert!(
        status["metered_usage"].get("stripe_reported").is_none(),
        "{status:?}"
    );
}
