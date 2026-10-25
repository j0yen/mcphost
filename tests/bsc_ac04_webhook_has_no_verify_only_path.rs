//! PRD-mcphost-billing-selfcheck-no-ledger-row AC4 -- no request field
//! selects the verify-only path: with a live key, signed `livemode: false`
//! deliveries (including one shaped exactly like the self-check event) both
//! return 200 and are ledgered as `.mode_mismatch`.

use crate::common::TestServer;
use mcphost::billing::{BillingConfig, FakeBillingClient, sign_for_test};
use serde_json::json;
use std::sync::Arc;

const SECRET: &str = "whsec_bsc_ac04";

#[tokio::test]
async fn bsc_ac04_self_check_shaped_delivery_is_ledgered_like_any_other() {
    let cfg = BillingConfig {
        secret_key: Some("sk_live_bsc04".to_string()),
        webhook_secret: Some(SECRET.to_string()),
        price_pro: None,
        metered_price_id: None,
        meter_event_name: None,
    };
    let server =
        TestServer::start_with_billing(cfg, Arc::new(FakeBillingClient::new(mcphost::state::now_unix()))).await;
    for (id, ty) in [
        ("evt_bsc04_1", "checkout.session.completed"),
        ("evt_status_self_check_1", "status.self_check"),
    ] {
        let body = serde_json::to_vec(&json!({
            "id": id, "type": ty, "livemode": false, "data": {"object": {}}
        }))
        .unwrap();
        let sig = sign_for_test(&body, SECRET, mcphost::state::now_unix());
        let resp = reqwest::Client::new()
            .post(format!("{}/billing/webhook", server.base_url))
            .header("Stripe-Signature", sig)
            .body(body)
            .send()
            .await
            .expect("POST /billing/webhook");
        assert_eq!(resp.status(), 200, "{id}");
    }
    let events = server.state.db.list_billing_events(None, None, None).await.expect("list");
    let mut types: Vec<&str> = events.iter().map(|e| e.event_type.as_str()).collect();
    types.sort();
    assert_eq!(
        types,
        ["checkout.session.completed.mode_mismatch", "status.self_check.mode_mismatch"],
        "{events:?}"
    );
}
