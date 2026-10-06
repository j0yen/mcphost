//! PRD-mcphost-upgrade-moment AC7 — Given two checkouts started of which
//! one completed, When `admin.upgrades --since 7d` runs, Then both are
//! listed with source, state and time to completion.

use crate::common;
use common::{ADMIN_KEY, McpClient, TestServer, signup};
use mcphost::billing::{BillingConfig, FakeBillingClient, sign_for_test};
use serde_json::json;
use std::sync::Arc;

const WEBHOOK_SECRET: &str = "whsec_test_upgrade_ac7";

fn billing_config() -> BillingConfig {
    BillingConfig {
        secret_key: Some("sk_test_upgrade_ac7".to_string()),
        webhook_secret: Some(WEBHOOK_SECRET.to_string()),
        price_pro: Some("price_pro_upgrade_ac7".to_string()),
        metered_price_id: None,
        meter_event_name: None,
    }
}

#[tokio::test]
async fn upgrades_lists_both_a_completed_and_a_pending_checkout() {
    let server = TestServer::start_with_billing(
        billing_config(),
        Arc::new(FakeBillingClient::new(mcphost::state::now_unix())),
    )
    .await;

    let (ns_a, key_a) = signup(&server.base_url, "Completes").await;
    let (ns_b, key_b) = signup(&server.base_url, "Still Pending").await;
    let client_a = McpClient::with_bearer(&server.base_url, &key_a);
    let client_b = McpClient::with_bearer(&server.base_url, &key_b);

    let checkout_a = client_a
        .tools_call("billing.checkout", json!({"plan": "pro", "source": "calls_per_day"}))
        .await
        .expect("tenant A checkout");
    let checkout_a = common::extract_structured(&checkout_a);
    let session_a_id = checkout_a["url"]
        .as_str()
        .expect("url")
        .rsplit('/')
        .next()
        .expect("session id in url")
        .to_string();

    client_b
        .tools_call("billing.checkout", json!({"plan": "pro", "source": "tools_max"}))
        .await
        .expect("tenant B checkout");

    // Complete tenant A's session: the webhook's own `data.object.id`
    // must equal the Checkout Session id `billing.checkout` minted (the
    // same `session_id` `admin.upgrades` correlates "started" with
    // "completed" by).
    let payload = json!({
        "id": "evt_ac7_completed",
        "type": "checkout.session.completed",
        "livemode": false,
        "data": {
            "object": {
                "id": session_a_id,
                "client_reference_id": ns_a,
                "customer": "cus_ac7",
                "amount_total": 1900,
                "currency": "usd",
                "metadata": {"source": "calls_per_day"},
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
    assert_eq!(resp.status(), 200);

    let admin = McpClient::with_bearer(&server.base_url, ADMIN_KEY);
    let result = admin
        .tools_call("admin.upgrades", json!({"days": 7}))
        .await
        .expect("admin.upgrades");
    let structured = common::extract_structured(&result);
    let upgrades = structured["upgrades"].as_array().expect("upgrades array");
    assert_eq!(upgrades.len(), 2, "{upgrades:?}");

    let entry_a = upgrades
        .iter()
        .find(|u| u["tenant"] == json!(ns_a))
        .unwrap_or_else(|| panic!("tenant A's checkout must be listed: {upgrades:?}"));
    assert_eq!(entry_a["source"], json!("calls_per_day"), "{entry_a:?}");
    assert_eq!(entry_a["state"], json!("completed"), "{entry_a:?}");
    assert!(
        entry_a["time_to_completion_s"].as_i64().is_some_and(|s| s >= 0),
        "{entry_a:?}"
    );

    let entry_b = upgrades
        .iter()
        .find(|u| u["tenant"] == json!(ns_b))
        .unwrap_or_else(|| panic!("tenant B's checkout must be listed: {upgrades:?}"));
    assert_eq!(entry_b["source"], json!("tools_max"), "{entry_b:?}");
    assert_eq!(entry_b["state"], json!("pending"), "{entry_b:?}");
    assert!(entry_b["time_to_completion_s"].is_null(), "{entry_b:?}");
}
