//! PRD-mcphost-billing-selfcheck-no-ledger-row AC3 -- when the ledger's
//! dedupe read errors (the `billing_events` table renamed away), the billing
//! sample is `ok: false` under both a test and a live key.

use crate::common::TestServer;
use mcphost::billing::{BillingConfig, FakeBillingClient};
use std::sync::Arc;

#[tokio::test]
async fn bsc_ac03_dedupe_read_error_fails_the_billing_sample() {
    for key in ["sk_test_bsc03", "sk_live_bsc03"] {
        let cfg = BillingConfig {
            secret_key: Some(key.to_string()),
            webhook_secret: Some("whsec_bsc_ac03".to_string()),
            price_pro: None,
            metered_price_id: None,
            meter_event_name: None,
        };
        let server =
            TestServer::start_with_billing(cfg, Arc::new(FakeBillingClient::new(mcphost::state::now_unix()))).await;
        let conn = rusqlite::Connection::open(server.data_dir.0.join("mcphost.db")).expect("raw db");
        conn.execute_batch("ALTER TABLE billing_events RENAME TO billing_events_gone")
            .expect("rename");
        drop(conn);
        mcphost::statusfeed::tick_once(&server.state).await.expect("tick_once");
        let sample = server
            .state
            .db
            .last_status_sample("billing".to_string())
            .await
            .expect("last sample")
            .expect("a billing sample");
        assert!(!sample.ok, "{key}: {sample:?}");
    }
}
