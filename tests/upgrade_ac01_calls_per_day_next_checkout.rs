//! PRD-mcphost-upgrade-moment AC1 — Given a free tenant at 500 calls
//! today, When it makes one more call, Then the refusal's
//! `error_data_json.next` is `{ tool: "billing.checkout", plan: "pro",
//! why: "calls_per_day 500/500", resets_at: <next window> }`.

use crate::common;
use common::{McpClient, TestServer, signup};
use mcphost::billing::{BillingConfig, FakeBillingClient};
use serde_json::json;
use std::sync::Arc;

fn echo_spec() -> serde_json::Value {
    json!({"schema": {"type": "object"}})
}

fn billing_config() -> BillingConfig {
    BillingConfig {
        secret_key: Some("sk_test_upgrade_ac1".to_string()),
        webhook_secret: Some("whsec_test_upgrade_ac1".to_string()),
        price_pro: Some("price_pro_upgrade_ac1".to_string()),
        metered_price_id: None,
        meter_event_name: None,
    }
}

#[tokio::test]
async fn the_501st_call_names_billing_checkout_as_the_next_step() {
    let server = TestServer::start_with_billing(
        billing_config(),
        Arc::new(FakeBillingClient::new(mcphost::state::now_unix())),
    )
    .await;
    let (ns, key) = signup(&server.base_url, "Heavy User").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "echoer", "kind": "echo", "spec": echo_spec()}),
        )
        .await
        .expect("publish must succeed");

    let tenant = server
        .state
        .db
        .find_tenant_by_namespace(ns.clone())
        .await
        .expect("db query")
        .expect("tenant must exist");

    // Pre-seed the free plan's 500 ok calls today directly through the db
    // handle `TestServer` exposes -- same convention
    // `billing_ac03_call_time_quota_exceeded.rs` already uses.
    for _ in 0..500 {
        server
            .state
            .db
            .record_call(tenant.id, "echoer".to_string(), 1, true, None, None, None, "ok", "external".to_string(), None)
            .await
            .expect("seed call");
    }

    let qualified = format!("{ns}.echoer");
    let err = client
        .tools_call(&qualified, json!({}))
        .await
        .expect_err("the 501st call today must be rejected");

    assert_eq!(err.error_code.as_deref(), Some("quota_exceeded"));
    let next = &err.data["next"];
    assert_eq!(next["tool"], json!("billing.checkout"), "{next:?}");
    assert_eq!(next["plan"], json!("pro"), "{next:?}");
    assert_eq!(next["why"], json!("calls_per_day 500/500"), "{next:?}");
    assert!(next["resets_at"].is_string(), "{next:?}");

    let midnight = mcphost::state::utc_midnight_unix(mcphost::state::now_unix());
    let expected_resets_at = mcphost::state::rfc3339_from_unix(midnight + 86_400);
    assert_eq!(next["resets_at"], json!(expected_resets_at), "{next:?}");
}
