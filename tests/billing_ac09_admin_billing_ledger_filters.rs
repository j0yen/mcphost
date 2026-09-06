//! AC9 — Given three ledgered events across two tenants, When an admin
//! calls `admin.billing_ledger` with `since` before them, Then all three
//! are returned newest first with `mode`, and `tenant` filtering returns
//! only that tenant's rows.

mod common;
use common::{ADMIN_KEY, McpClient, TestServer, signup};
use mcphost::billing::{BillingConfig, FakeBillingClient, sign_for_test};
use serde_json::json;
use std::sync::Arc;

const WEBHOOK_SECRET: &str = "whsec_test_ac9";

fn billing_config() -> BillingConfig {
    BillingConfig {
        secret_key: Some("sk_test_ac9".to_string()),
        webhook_secret: Some(WEBHOOK_SECRET.to_string()),
        price_pro: Some("price_pro_ac9".to_string()),
        metered_price_id: None,
        meter_event_name: None,
    }
}

async fn send_checkout_completed(
    server: &common::TestServer,
    event_id: &str,
    ns: &str,
    customer: &str,
) {
    let payload = json!({
        "id": event_id,
        "type": "checkout.session.completed",
        "livemode": false,
        "data": {
            "object": {
                "client_reference_id": ns,
                "customer": customer,
                "amount_total": 2900,
                "currency": "usd",
            }
        }
    });
    let body = serde_json::to_vec(&payload).unwrap();
    let sig = sign_for_test(&body, WEBHOOK_SECRET, mcphost::state::now_unix());
    let resp = reqwest::Client::new()
        .post(format!("{}/billing/webhook", server.base_url))
        .header("Stripe-Signature", sig)
        .body(body)
        .send()
        .await
        .expect("POST /billing/webhook");
    assert_eq!(resp.status(), 200);
}

#[tokio::test]
async fn billing_ledger_lists_events_newest_first_and_filters_by_tenant() {
    let server = TestServer::start_with_billing(
        billing_config(),
        Arc::new(FakeBillingClient::new(mcphost::state::now_unix())),
    )
    .await;
    let before = mcphost::state::now_unix() - 5;
    let (ns_a, _) = signup(&server.base_url, "Tenant A").await;
    let (ns_b, _) = signup(&server.base_url, "Tenant B").await;

    send_checkout_completed(&server, "evt_ac9_1", &ns_a, "cus_ac9_a1").await;
    send_checkout_completed(&server, "evt_ac9_2", &ns_a, "cus_ac9_a2").await;
    send_checkout_completed(&server, "evt_ac9_3", &ns_b, "cus_ac9_b1").await;

    let admin = McpClient::with_bearer(&server.base_url, ADMIN_KEY);
    let result = admin
        .tools_call("admin.billing_ledger", json!({"since": before}))
        .await
        .expect("admin.billing_ledger");
    let structured = common::extract_structured(&result);
    let events = structured["events"].as_array().expect("events array");
    assert_eq!(events.len(), 3, "{events:?}");
    let ids: Vec<&str> = events
        .iter()
        .map(|e| e["event_id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, vec!["evt_ac9_3", "evt_ac9_2", "evt_ac9_1"], "newest first");
    for e in events {
        assert_eq!(e["mode"], json!("test"));
    }

    let filtered = admin
        .tools_call("admin.billing_ledger", json!({"tenant": ns_a}))
        .await
        .expect("admin.billing_ledger filtered by tenant");
    let filtered_events = common::extract_structured(&filtered)["events"].clone();
    let filtered_events = filtered_events.as_array().unwrap();
    assert_eq!(filtered_events.len(), 2, "{filtered_events:?}");
    assert!(filtered_events.iter().all(|e| e["tenant"] == json!(ns_a)));
}
