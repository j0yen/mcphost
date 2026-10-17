//! PRD-mcphost-sandbox-pool-self-restart AC4 (P0): with two `ok` restart rows
//! in the last 3600 s a third breach writes a `rate_limited` row, kills no
//! child, and records `status.component_down` for `exec`.

use crate::common;

use std::sync::Arc;
use std::time::Duration;

use mcphost::kinds::python::PythonKind;

#[tokio::test]
async fn third_breach_in_the_hour_is_rate_limited_kills_nothing_and_raises_exec_down() {
    let _live = common::EnvGuard::clear(mcphost::heal::DRY_RUN_ENV).await;
    let (mut state, data_dir) = common::bare_app_state().await;
    let python = Arc::new(PythonKind::for_test_with_warm_pool(&data_dir.0, Duration::from_secs(60), 4, 8));
    state.kinds.register(python.clone());

    for n in 0..2 {
        let row = mcphost::pool_restart::run(&state, &common::pool_breach()).await.expect("restart").expect("python kind");
        assert_eq!(row.outcome.as_str(), "ok", "restart {n}");
    }

    let timeout = Duration::from_millis(100);
    let mut wedged = common::WedgedChild::spawn(b"");
    let _planted = python.hold_test_flight_with_cancel("7/wedged_tool", timeout, timeout * 4, Some(wedged.slot.clone()));

    let row = mcphost::pool_restart::run(&state, &common::pool_breach()).await.expect("restart").expect("python kind");
    assert_eq!(row.outcome.as_str(), "rate_limited");
    assert!(!wedged.is_dead().await, "a rate-limited restart kills no child");
    assert_eq!(python.wedged(std::time::Instant::now()).len(), 1, "the wedge is still there");

    mcphost::statusfeed::tick_once(&state).await.expect("status tick");
    let alerts = state.db.list_alerts(100, None, false).await.expect("list_alerts");
    let down: Vec<_> = alerts.iter().filter(|a| a.key == "status.component_down").collect();
    assert!(
        down.iter().any(|a| a.body_json.contains("\"exec\"")),
        "status.component_down must be recorded for exec: {:?}",
        alerts.iter().map(|a| (&a.key, &a.body_json)).collect::<Vec<_>>()
    );
}
