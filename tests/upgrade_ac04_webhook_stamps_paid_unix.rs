//! PRD-mcphost-upgrade-moment AC4 — Given a live-mode
//! `checkout.session.completed` webhook for a tenant with `paid_unix`
//! null, When it is processed, Then `paid_unix` is set,
//! `billing_events.source` equals the session metadata, and a second
//! identical webhook changes nothing; given test mode, Then
//! `paid_test_unix` is set and `paid_unix` stays null.

use crate::common;
use common::{TestServer, signup};
use mcphost::billing::{BillingConfig, FakeBillingClient, sign_for_test};
use serde_json::json;
use std::sync::Arc;

const WEBHOOK_SECRET: &str = "whsec_test_upgrade_ac4";

fn billing_config() -> BillingConfig {
    BillingConfig {
        secret_key: Some("sk_live_upgrade_ac4".to_string()),
        webhook_secret: Some(WEBHOOK_SECRET.to_string()),
        price_pro: Some("price_pro_upgrade_ac4".to_string()),
        metered_price_id: None,
        meter_event_name: None,
    }
}

#[allow(clippy::too_many_arguments)]
async fn post_checkout_completed(
    server: &common::TestServer,
    event_id: &str,
    ns: &str,
    customer: &str,
    livemode: bool,
    source: &str,
) -> reqwest::StatusCode {
    let payload = json!({
        "id": event_id,
        "type": "checkout.session.completed",
        "livemode": livemode,
        "data": {
            "object": {
                "id": format!("cs_{event_id}"),
                "client_reference_id": ns,
                "customer": customer,
                "subscription": format!("sub_{event_id}"),
                "amount_total": 2900,
                "currency": "usd",
                "metadata": {"source": source},
            }
        }
    });
    let body = serde_json::to_vec(&payload).unwrap();
    let now = mcphost::state::now_unix();
    let signature = sign_for_test(&body, WEBHOOK_SECRET, now);
    let resp = reqwest::Client::new()
        .post(format!("{}/billing/webhook", server.base_url))
        .header("Stripe-Signature", &signature)
        .header("Content-Type", "application/json")
        .body(body)
        .send()
        .await
        .expect("POST /billing/webhook");
    resp.status()
}

#[tokio::test]
async fn live_mode_checkout_completed_stamps_paid_unix_once() {
    // `sk_live_...` -- the host's own configured key is live mode, so a
    // `livemode: true` event agrees with it (AC10's mode-mismatch guard
    // only rejects a DISAGREEMENT between the two).
    let server = TestServer::start_with_billing(
        billing_config(),
        Arc::new(FakeBillingClient::new(mcphost::state::now_unix())),
    )
    .await;
    let (ns, _key) = signup(&server.base_url, "Soon Paying").await;
    let tenant = server
        .state
        .db
        .find_tenant_by_namespace(ns.clone())
        .await
        .expect("db query")
        .expect("tenant must exist");
    assert!(tenant.paid_unix.is_none(), "must start unpaid");

    let status = post_checkout_completed(
        &server,
        "evt_ac4_live_1",
        &ns,
        "cus_ac4_live",
        true,
        "calls_per_day",
    )
    .await;
    assert_eq!(status, 200);

    let tenant = server
        .state
        .db
        .find_tenant_by_namespace(ns.clone())
        .await
        .expect("db query")
        .expect("tenant must exist");
    assert!(tenant.paid_unix.is_some(), "paid_unix must now be set");
    assert!(tenant.paid_test_unix.is_none(), "paid_test_unix must stay null in live mode");
    let first_paid_unix = tenant.paid_unix;

    let events = server
        .state
        .db
        .list_billing_events(None, None, Some(ns.clone()))
        .await
        .expect("list_billing_events");
    assert_eq!(events.len(), 1, "{events:?}");
    assert_eq!(events[0].source.as_deref(), Some("calls_per_day"));

    // A second identical webhook (same event_id) changes nothing: no new
    // ledger row, and paid_unix is untouched (not re-stamped to a later
    // time).
    let status2 = post_checkout_completed(
        &server,
        "evt_ac4_live_1",
        &ns,
        "cus_ac4_live",
        true,
        "calls_per_day",
    )
    .await;
    assert_eq!(status2, 200);

    let tenant_after = server
        .state
        .db
        .find_tenant_by_namespace(ns.clone())
        .await
        .expect("db query")
        .expect("tenant must exist");
    assert_eq!(tenant_after.paid_unix, first_paid_unix, "paid_unix must not change");

    let events_after = server
        .state
        .db
        .list_billing_events(None, None, Some(ns))
        .await
        .expect("list_billing_events after duplicate");
    assert_eq!(events_after.len(), 1, "duplicate delivery must not add a row");
}

#[tokio::test]
async fn test_mode_checkout_completed_stamps_paid_test_unix_not_paid_unix() {
    // `livemode: false` mismatches a live-mode key (AC10's mode-mismatch
    // guard), so this host is configured for test mode instead -- the
    // event's own `livemode` then agrees with it and is actually applied.
    let test_mode_config = BillingConfig {
        secret_key: Some("sk_test_upgrade_ac4".to_string()),
        webhook_secret: Some(WEBHOOK_SECRET.to_string()),
        price_pro: Some("price_pro_upgrade_ac4".to_string()),
        metered_price_id: None,
        meter_event_name: None,
    };
    let test_server = TestServer::start_with_billing(
        test_mode_config,
        Arc::new(FakeBillingClient::new(mcphost::state::now_unix())),
    )
    .await;
    let (test_ns, _test_key) = signup(&test_server.base_url, "Fleet Test Tenant").await;

    let status = post_checkout_completed(
        &test_server,
        "evt_ac4_test_1",
        &test_ns,
        "cus_ac4_test",
        false,
        "manual",
    )
    .await;
    assert_eq!(status, 200);

    let tenant = test_server
        .state
        .db
        .find_tenant_by_namespace(test_ns)
        .await
        .expect("db query")
        .expect("tenant must exist");
    assert!(tenant.paid_test_unix.is_some(), "paid_test_unix must be set");
    assert!(tenant.paid_unix.is_none(), "paid_unix must stay null for a test-mode checkout");
}
