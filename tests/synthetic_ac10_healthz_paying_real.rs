//! PRD-mcphost-synthetic-flag
//! AC10 (P1) — Given one labeled tenant on a paid plan, When `/healthz` is
//! read, Then `paying_tenants_real` is present and excludes it.

mod common;
use common::{ADMIN_KEY, McpClient, TestServer, signup};
use mcphost::billing::{BillingConfig, FakeBillingClient};
use serde_json::json;
use std::sync::Arc;

// PRD-mcphost-healthz-minimal: the full diagnostics document (including
// `paying_tenants`/`paying_tenants_real`) is gated behind the admin bearer
// -- an unauthenticated GET gets only `{"ok": true/false}`.
async fn healthz(base_url: &str) -> serde_json::Value {
    reqwest::Client::new()
        .get(format!("{base_url}/healthz"))
        .bearer_auth(ADMIN_KEY)
        .send()
        .await
        .expect("GET /healthz")
        .json()
        .await
        .expect("parse /healthz")
}

#[tokio::test]
async fn absent_until_a_synthetic_tenant_pays_then_excludes_it() {
    let billing_config = BillingConfig {
        secret_key: Some("sk_test_synth_ac10".to_string()),
        webhook_secret: Some("whsec_test_synth_ac10".to_string()),
        price_pro: Some("price_pro_synth_ac10".to_string()),
        metered_price_id: None,
        meter_event_name: None,
    };
    let server = TestServer::start_with_billing(
        billing_config,
        Arc::new(FakeBillingClient::new(mcphost::state::now_unix())),
    )
    .await;
    let admin = McpClient::with_bearer(&server.base_url, ADMIN_KEY);

    let (ns_real, _) = signup(&server.base_url, "Real Payer").await;
    admin
        .tools_call(
            "admin.plan_set",
            json!({"tenant": ns_real, "plan": "pro", "reason": "AC10 setup"}),
        )
        .await
        .expect("plan_set real payer");

    // No synthetic tenant has ever paid yet: the field must be absent, not
    // present-and-equal-to-paying_tenants.
    let health_before = healthz(&server.base_url).await;
    assert!(
        health_before.get("paying_tenants_real").is_none(),
        "paying_tenants_real must be absent with no synthetic payer: {health_before:?}"
    );
    assert_eq!(health_before["paying_tenants"], json!(1), "{health_before:?}");

    let (ns_synthetic, _) = signup(&server.base_url, "Synthetic Payer").await;
    admin
        .tools_call(
            "admin.tenant_set_synthetic",
            json!({"tenant": ns_synthetic, "label": "synthorg:ac10"}),
        )
        .await
        .expect("label synthetic payer");
    admin
        .tools_call(
            "admin.plan_set",
            json!({"tenant": ns_synthetic, "plan": "pro", "reason": "AC10 setup"}),
        )
        .await
        .expect("plan_set synthetic payer");

    let health_after = healthz(&server.base_url).await;
    assert_eq!(health_after["paying_tenants"], json!(2), "{health_after:?}");
    assert_eq!(
        health_after["paying_tenants_real"],
        json!(1),
        "paying_tenants_real must exclude the synthetic payer: {health_after:?}"
    );
}
