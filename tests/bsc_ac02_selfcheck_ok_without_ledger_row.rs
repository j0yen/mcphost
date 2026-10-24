//! PRD-mcphost-billing-selfcheck-no-ledger-row AC2 -- with a live key and a
//! webhook secret, one real-probe tick records `billing` ok/self and leaves
//! no `evt_status_self_check_` row in the ledger.

use crate::common::TestServer;
use mcphost::billing::{BillingConfig, FakeBillingClient};
use std::sync::Arc;

#[tokio::test]
async fn bsc_ac02_live_key_tick_is_ok_and_ledgers_nothing() {
    let cfg = BillingConfig {
        secret_key: Some("sk_live_bsc02".to_string()),
        webhook_secret: Some("whsec_bsc_ac02".to_string()),
        price_pro: None,
        metered_price_id: None,
        meter_event_name: None,
    };
    let server =
        TestServer::start_with_billing(cfg, Arc::new(FakeBillingClient::new(mcphost::state::now_unix()))).await;
    mcphost::statusfeed::tick_once(&server.state).await.expect("tick_once");
    let sample = server
        .state
        .db
        .last_status_sample("billing".to_string())
        .await
        .expect("last sample")
        .expect("a billing sample");
    assert!(sample.ok, "{sample:?}");
    assert_eq!(sample.source, "self");
    let events = server.state.db.list_billing_events(None, None, None).await.expect("list");
    assert!(
        events.iter().all(|e| !e.event_id.starts_with("evt_status_self_check_")),
        "{events:?}"
    );
}
