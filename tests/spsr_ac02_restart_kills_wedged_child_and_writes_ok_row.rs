//! PRD-mcphost-sandbox-pool-self-restart AC2 (P0): on a breach with
//! `MCPHOST_HEAL_DRY_RUN` unset the wedged child is killed, the pool reads
//! `state: "ready", wedged: 0` afterwards, and the ledger row is
//! `sandbox_pool_restart` / `ok` with `before.wedged: 1`, `after.wedged: 0`.

use crate::common;

use std::sync::Arc;
use std::time::{Duration, Instant};

use mcphost::kinds::python::PythonKind;
use serde_json::{Value, json};

#[tokio::test]
async fn breach_kills_the_wedged_child_and_the_row_reads_ok_with_before_and_after() {
    let _live = common::EnvGuard::clear(mcphost::heal::DRY_RUN_ENV).await;
    let (mut state, data_dir) = common::bare_app_state().await;
    let python = Arc::new(PythonKind::for_test_with_warm_pool(&data_dir.0, Duration::from_secs(60), 4, 8));
    state.kinds.register(python.clone());

    let timeout = Duration::from_millis(100);
    let mut wedged = common::WedgedChild::spawn(b"");
    let _planted = python.hold_test_flight_with_cancel("7/wedged_tool", timeout, timeout * 4, Some(wedged.slot.clone()));
    assert_eq!(python.wedged(Instant::now()).len(), 1);

    let row = mcphost::pool_restart::run(&state, &common::pool_breach()).await.expect("restart").expect("python kind");
    assert!(wedged.is_dead().await, "the wedged child must be killed");

    let stored = state.db.find_heal_row(row.id).await.expect("read").expect("row");
    assert_eq!(stored.action.as_str(), "sandbox_pool_restart");
    assert_eq!(stored.outcome.as_str(), "ok");
    assert!(!stored.dry_run);
    assert_eq!(stored.before["wedged"], json!(1), "before: {}", stored.before);
    assert_eq!(stored.after["wedged"], json!(0), "after: {}", stored.after);

    let status = mcphost::statusfeed::status_json(&state).await.expect("status_json");
    let pool: &Value = &status["sandbox_pool"];
    assert_eq!(pool["state"], json!("ready"), "sandbox_pool: {pool}");
    assert_eq!(pool["wedged"], json!(0), "sandbox_pool: {pool}");
    assert_eq!(pool["last_restart"]["outcome"], json!("ok"));
    assert_eq!(pool["last_restart"]["heal_action_id"], json!(row.id));
}
