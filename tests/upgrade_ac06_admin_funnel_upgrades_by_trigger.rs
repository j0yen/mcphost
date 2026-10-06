//! PRD-mcphost-upgrade-moment AC6 — Given three paid tenants with sources
//! calls_per_day, calls_per_day, tools_max, When `admin.funnel` runs,
//! Then `paid 3` and `upgrades_by_trigger { calls_per_day: 2, tools_max: 1 }`.

use crate::common;
use common::{ADMIN_KEY, McpClient, TestServer, signup};
use mcphost::billing::{BillingConfig, FakeBillingClient, sign_for_test};
use serde_json::json;
use std::sync::Arc;

const WEBHOOK_SECRET: &str = "whsec_test_upgrade_ac6";

fn billing_config() -> BillingConfig {
    BillingConfig {
        // `sk_live_...` -- a `livemode: true` event agrees with this
        // host's own configured key, so it is actually applied rather
        // than ledgered as a `.mode_mismatch` (AC10).
        secret_key: Some("sk_live_upgrade_ac6".to_string()),
        webhook_secret: Some(WEBHOOK_SECRET.to_string()),
        price_pro: Some("price_pro_upgrade_ac6".to_string()),
        metered_price_id: None,
        meter_event_name: None,
    }
}

async fn pay_tenant(server: &common::TestServer, event_id: &str, ns: &str, source: &str) {
    let payload = json!({
        "id": event_id,
        "type": "checkout.session.completed",
        "livemode": true,
        "data": {
            "object": {
                "id": format!("cs_{event_id}"),
                "client_reference_id": ns,
                "customer": format!("cus_{event_id}"),
                "amount_total": 1900,
                "currency": "usd",
                "metadata": {"source": source},
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
    assert_eq!(resp.status(), 200, "{source} webhook must be accepted");
}

#[tokio::test]
async fn funnel_reports_paid_count_and_upgrades_by_trigger() {
    let server = TestServer::start_with_billing(
        billing_config(),
        Arc::new(FakeBillingClient::new(mcphost::state::now_unix())),
    )
    .await;

    let (ns_a, _) = signup(&server.base_url, "Tenant A").await;
    let (ns_b, _) = signup(&server.base_url, "Tenant B").await;
    let (ns_c, _) = signup(&server.base_url, "Tenant C").await;

    pay_tenant(&server, "evt_ac6_a", &ns_a, "calls_per_day").await;
    pay_tenant(&server, "evt_ac6_b", &ns_b, "calls_per_day").await;
    pay_tenant(&server, "evt_ac6_c", &ns_c, "tools_max").await;

    let admin = McpClient::with_bearer(&server.base_url, ADMIN_KEY);
    let result = admin.tools_call("admin.funnel", json!({})).await.expect("admin.funnel");
    let structured = common::extract_structured(&result);

    let stages = structured["stages"].as_array().expect("stages array");
    let paid_stage = stages
        .iter()
        .find(|s| s["name"] == json!("paid"))
        .expect("paid stage present");
    assert_eq!(paid_stage["count"], json!(3), "{paid_stage:?}");

    assert_eq!(
        structured["upgrades_by_trigger"],
        json!({"calls_per_day": 2, "tools_max": 1}),
        "{structured:?}"
    );
}
