//! AC11 — Given `/healthz` after two tenants are `pro`, When it is read,
//! Then `paying_tenants: 2` and `billing_mode` reflects the key prefix.

mod common;
use common::{ADMIN_KEY, McpClient, TestServer, signup};
use mcphost::billing::{BillingConfig, FakeBillingClient};
use serde_json::json;
use std::sync::Arc;

#[tokio::test]
async fn healthz_reports_paying_tenants_and_billing_mode() {
    let billing_config = BillingConfig {
        secret_key: Some("sk_test_ac11".to_string()),
        webhook_secret: Some("whsec_test_ac11".to_string()),
        price_pro: Some("price_pro_ac11".to_string()),
        metered_price_id: None,
        meter_event_name: None,
    };
    let server = TestServer::start_with_billing(
        billing_config,
        Arc::new(FakeBillingClient::new(mcphost::state::now_unix())),
    )
    .await;
    let (ns_a, _) = signup(&server.base_url, "Payer A").await;
    let (ns_b, _) = signup(&server.base_url, "Payer B").await;
    let (_ns_c, _) = signup(&server.base_url, "Free C").await;

    let admin = McpClient::with_bearer(&server.base_url, ADMIN_KEY);
    for ns in [&ns_a, &ns_b] {
        admin
            .tools_call(
                "admin.plan_set",
                json!({"tenant": ns, "plan": "pro", "reason": "AC11 setup"}),
            )
            .await
            .expect("admin.plan_set");
    }

    let health: serde_json::Value = reqwest::get(format!("{}/healthz", server.base_url))
        .await
        .expect("GET /healthz")
        .json()
        .await
        .expect("parse /healthz");
    assert_eq!(health["paying_tenants"], json!(2), "{health:?}");
    assert_eq!(health["billing_mode"], json!("test"), "{health:?}");
}
