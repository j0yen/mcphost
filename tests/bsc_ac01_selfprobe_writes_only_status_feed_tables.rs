//! PRD-mcphost-billing-selfcheck-no-ledger-row AC1 -- with the real probes
//! (no `status_probe_override`), three status-feed ticks change no row count
//! in any table outside `STATUS_FEED_TABLES`, in every billing mode.
//! (`exec` is not exercised: the default `TestServer` registers no `python`
//! kind, so `probe_exec` returns before calling.)

use crate::common::TestServer;
use mcphost::billing::{BillingConfig, FakeBillingClient};
use mcphost::statusfeed::{self, STATUS_FEED_TABLES};
use std::collections::BTreeMap;
use std::sync::Arc;

fn cfg(secret_key: Option<&str>) -> BillingConfig {
    BillingConfig {
        secret_key: secret_key.map(str::to_string),
        webhook_secret: Some("whsec_bsc_ac01".to_string()),
        price_pro: None,
        metered_price_id: None,
        meter_event_name: None,
    }
}

fn counts(server: &TestServer) -> BTreeMap<String, i64> {
    let conn = rusqlite::Connection::open(server.data_dir.0.join("mcphost.db")).expect("raw db");
    let names: Vec<String> = conn
        .prepare("SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%'")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    names
        .into_iter()
        .map(|n| {
            let c: i64 = conn
                .query_row(&format!("SELECT COUNT(*) FROM \"{n}\""), [], |r| r.get(0))
                .unwrap();
            (n, c)
        })
        .collect()
}

#[tokio::test]
async fn bsc_ac01_ticks_change_no_table_outside_the_status_feed_set() {
    assert!(!STATUS_FEED_TABLES.contains(&"billing_events"));
    for (mode, key) in [("off", None), ("test", Some("sk_test_bsc")), ("live", Some("sk_live_bsc"))] {
        let server =
            TestServer::start_with_billing(cfg(key), Arc::new(FakeBillingClient::new(mcphost::state::now_unix())))
                .await;
        let before = counts(&server);
        for _ in 0..3 {
            statusfeed::tick_once(&server.state).await.expect("tick_once");
        }
        let after = counts(&server);
        for (table, n) in &before {
            if STATUS_FEED_TABLES.contains(&table.as_str()) {
                continue;
            }
            assert_eq!(after.get(table), Some(n), "mode {mode}: table {table} changed across 3 ticks");
        }
    }
}
