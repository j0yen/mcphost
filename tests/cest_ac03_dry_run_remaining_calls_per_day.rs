//! PRD-mcphost-call-cost-estimate
//! AC3 -- Given a free tenant at 499 of 500 calls, When it dry-runs a call,
//! Then `fits: true`, `remaining.calls_per_day.remaining` is 1 and
//! `resets_at` equals the next UTC midnight; Given 500 of 500, Then
//! `fits: false` and `refusal.code` is `quota_exceeded` with `next.tool`
//! `billing.checkout`.

use crate::common;
use common::{McpClient, TestServer, extract_structured, record_ok_calls, signup};
use mcphost::billing::{BillingConfig, FakeBillingClient};
use serde_json::json;
use std::sync::Arc;

fn billing_config() -> BillingConfig {
    BillingConfig {
        secret_key: Some("sk_test_cest_ac3".to_string()),
        webhook_secret: Some("whsec_test_cest_ac3".to_string()),
        price_pro: Some("price_pro_cest_ac3".to_string()),
        metered_price_id: None,
        meter_event_name: None,
    }
}

#[tokio::test]
async fn dry_run_reports_one_call_left_then_the_checkout_refusal() {
    let server = TestServer::start_with_billing(
        billing_config(),
        Arc::new(FakeBillingClient::new(mcphost::state::now_unix())),
    )
    .await;
    let (ns, key) = signup(&server.base_url, "Cest Quota").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "echoer", "kind": "echo", "spec": {"schema": {"type": "object"}}}),
        )
        .await
        .expect("publish");
    let tenant = server.state.db.find_tenant_by_namespace(ns).await.expect("db").expect("tenant");
    let next_midnight = mcphost::state::rfc3339_from_unix(
        mcphost::state::utc_midnight_unix(mcphost::state::now_unix()) + 86_400,
    );

    record_ok_calls(&server, tenant.id, "echoer", 499).await;
    let dry = extract_structured(
        &client
            .tools_call("host.tool_call", json!({"name": "echoer", "args": {}, "dry_run": true}))
            .await
            .expect("dry run at 499/500"),
    );
    assert_eq!(dry["fits"], json!(true), "{dry}");
    assert_eq!(dry["refusal"], serde_json::Value::Null, "{dry}");
    let daily = &dry["remaining"]["calls_per_day"];
    assert_eq!(daily["remaining"], json!(1), "{dry}");
    assert_eq!(daily["used"], json!(499), "{dry}");
    assert_eq!(daily["limit"], json!(500), "{dry}");
    assert_eq!(daily["resets_at"], json!(next_midnight), "{dry}");
    assert_eq!(dry["units"], json!({"calls": 1}), "{dry}");

    record_ok_calls(&server, tenant.id, "echoer", 1).await;
    let dry = extract_structured(
        &client
            .tools_call("host.tool_call", json!({"name": "echoer", "args": {}, "dry_run": true}))
            .await
            .expect("dry run at 500/500 answers, it does not error"),
    );
    assert_eq!(dry["fits"], json!(false), "{dry}");
    assert_eq!(dry["refusal"]["code"], json!("quota_exceeded"), "{dry}");
    assert_eq!(dry["refusal"]["data"]["next"]["tool"], json!("billing.checkout"), "{dry}");
    assert_eq!(dry["remaining"]["calls_per_day"]["remaining"], json!(0), "{dry}");
}
