//! PRD-mcphost-sandbox-pool-self-restart-v2 AC4: with `max_per_hour` ok rows
//! (read from `HealAction::SandboxPoolRestart.policy()`) in the last 3600 s,
//! another breach writes a `rate_limited` row, kills no child, raises
//! `status.component_down`, and leaves exactly one open incident naming
//! `exec` with `auto` false -- still open after the next status tick, even
//! though `exec` reads ok (the probe makes its own cold call).

use crate::common::EnvGuard;
use crate::spsr;

use mcphost::heal::{self, HealAction};
use mcphost::state::now_unix;
use serde_json::json;

#[tokio::test]
async fn spsr_ac04() {
    if mcphost::sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", mcphost::sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let _dry = EnvGuard::clear(heal::DRY_RUN_ENV).await;
    let (state, python, _dir) = spsr::app(5).await;
    let wedge = spsr::wedge(&python, "stalled_tool").await;

    let limit = HealAction::SandboxPoolRestart.policy().max_per_hour;
    for i in 0..limit {
        let row = heal::execute_on(
            &state.db,
            HealAction::SandboxPoolRestart,
            json!({"signal": "sandbox_pool"}),
            now_unix() - 60 - i64::from(i),
            false,
            serde_json::Value::Null,
            || async { Ok(json!({"before": {"wedged": 1}, "after": {"wedged": 0}})) },
        )
        .await
        .unwrap();
        assert_eq!(row.outcome.as_str(), "ok");
    }
    let spent = state.db.heal_executed_since("sandbox_pool_restart".to_string(), now_unix() - heal::RATE_WINDOW_SECS).await.unwrap();
    assert_eq!(spent, i64::from(limit));

    let _subscriber = mcphost::pool_restart::spawn_subscriber(state.clone());
    spsr::breach(&state);
    let row = spsr::wait_row_after(&state, i64::from(limit)).await;

    assert_eq!(row.outcome.as_str(), "rate_limited", "row: {row:?}");
    assert!(spsr::group_alive(wedge.pgid), "no child was killed");
    assert!(!wedge.call.is_finished(), "the stalled call is still blocked");

    let alerts = state.db.list_alerts(50, None, false).await.unwrap();
    assert!(alerts.iter().any(|a| a.key == "status.component_down"), "alerts: {alerts:?}");
    let open_exec = |incidents: &[mcphost::db::Incident]| {
        incidents.iter().filter(|i| i.components_json.contains("\"exec\"")).map(|i| i.auto).collect::<Vec<_>>()
    };
    assert_eq!(open_exec(&state.db.list_open_incidents().await.unwrap()), vec![false]);

    // Nine ok samples already stand; the next tick's real `exec` probe reads
    // ok and makes ten, which closes an `auto` incident -- not this one.
    for i in 1..=9 {
        state.db.insert_status_sample("exec".to_string(), now_unix() - 10 * i, true, 5, "self".to_string()).await.unwrap();
    }
    mcphost::statusfeed::tick_once(&state).await.unwrap();
    let recent = state.db.recent_status_samples("exec".to_string(), 10).await.unwrap();
    assert!(recent.iter().take(10).all(|s| s.ok), "exec reads ok: {recent:?}");
    assert_eq!(open_exec(&state.db.list_open_incidents().await.unwrap()), vec![false], "still open after the tick");

    wedge.call.abort();
}
