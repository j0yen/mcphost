//! PRD-mcphost-sandbox-pool-self-restart AC5 (P0): with
//! `MCPHOST_HEAL_DRY_RUN=1` a breach writes a `would_run` row with
//! `trigger.wedged_keys` = 1, the planted child stays alive, and
//! `/status.json.sandbox_pool.state` stays `ready`.

use crate::common;

use std::sync::Arc;
use std::time::Duration;

use mcphost::kinds::python::PythonKind;
use serde_json::json;

#[tokio::test]
async fn dry_run_breach_writes_would_run_naming_one_wedge_and_leaves_the_pool_alone() {
    let _dry_run = common::EnvGuard::set(mcphost::heal::DRY_RUN_ENV, "1").await;
    let (mut state, data_dir) = common::bare_app_state().await;
    let python = Arc::new(PythonKind::for_test_with_warm_pool(&data_dir.0, Duration::from_secs(60), 4, 8));
    state.kinds.register(python.clone());

    let timeout = Duration::from_millis(100);
    let mut wedged = common::WedgedChild::spawn(b"");
    let _planted = python.hold_test_flight_with_cancel("7/wedged_tool", timeout, timeout * 4, Some(wedged.slot.clone()));

    let row = mcphost::pool_restart::run(&state, &common::pool_breach()).await.expect("restart").expect("python kind");
    assert_eq!(row.outcome.as_str(), "would_run");
    assert!(row.dry_run);

    let stored = state.db.find_heal_row(row.id).await.expect("read").expect("row");
    assert_eq!(stored.outcome.as_str(), "would_run");
    assert_eq!(stored.trigger["wedged_keys"], json!(1), "trigger: {}", stored.trigger);
    assert!(!wedged.is_dead().await, "a dry run kills nothing");

    let status = mcphost::statusfeed::status_json(&state).await.expect("status_json");
    assert_eq!(status["sandbox_pool"]["state"], json!("ready"), "sandbox_pool: {}", status["sandbox_pool"]);
    assert_eq!(status["sandbox_pool"]["wedged"], json!(1));
}
