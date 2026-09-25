//! PRD-mcphost-sqlite-busy-timeout-audit
//! AC10 — Given `MCPHOST_DB_WAL_CHECKPOINT_SECS=1`, When the cron loop
//! ticks, Then a passive checkpoint runs and
//! `admin.db.stats.last_checkpoint` updates.
//!
//! `MCPHOST_DB_WAL_CHECKPOINT_SECS=1` is exercised as a pure parse (`Db::
//! parse_wal_checkpoint_secs`) rather than a real process env var, which
//! would race every other test in the same suite binary reading the same
//! var (`Db::open_with_cfg`'s own doc comment gives the identical
//! rationale for `MCPHOST_DB_BUSY_TIMEOUT_MS`). The cron loop itself is
//! driven via `db::spawn_wal_checkpoint_scheduler_for_test` with a short
//! real interval, same "swap a short interval in for the test" convention
//! `retention::spawn_prune_scheduler_for_test` already uses.

use crate::common;
use common::{ADMIN_KEY, McpClient, TestServer, extract_structured};
use serde_json::json;

#[test]
fn wal_checkpoint_secs_env_var_parses_to_one() {
    assert_eq!(mcphost::db::parse_wal_checkpoint_secs(Some("1")), 1);
    assert_eq!(mcphost::db::parse_wal_checkpoint_secs(None), 300);
}

#[tokio::test]
async fn cron_tick_runs_a_passive_checkpoint_and_updates_admin_db_stats() {
    let server = TestServer::start().await;
    let admin = McpClient::with_bearer(&server.base_url, ADMIN_KEY);

    let before = extract_structured(
        &admin
            .tools_call("admin.db.stats", json!({}))
            .await
            .expect("admin.db.stats before"),
    );
    assert_eq!(before["last_checkpoint"], serde_json::Value::Null, "{before:?}");

    let _scheduler = mcphost::db::spawn_wal_checkpoint_scheduler_for_test(
        (*server.state).clone(),
        std::time::Duration::from_millis(50),
    );
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    let after = extract_structured(
        &admin
            .tools_call("admin.db.stats", json!({}))
            .await
            .expect("admin.db.stats after"),
    );
    assert!(
        after["last_checkpoint"].is_string(),
        "last_checkpoint must be set after the cron loop ticks: {after:?}"
    );
}
