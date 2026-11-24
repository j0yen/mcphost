//! PRD-mcphost-sandbox-pool-self-restart-v2 AC3: a healthy call that cannot
//! finish inside `drain_budget_s` fails `tool_timeout` when the drain ends,
//! with `data.heal_action_id` equal to the ledger row id and
//! `data.retry_after_ms` above 0; a call that finishes inside the budget
//! returns `ok`.

use crate::common::EnvGuard;
use crate::spsr;

use mcphost::kinds::KindError;
use serde_json::json;

#[tokio::test]
async fn spsr_ac03() {
    if mcphost::sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", mcphost::sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let _dry = EnvGuard::clear(mcphost::heal::DRY_RUN_ENV).await;
    let (state, python, _dir) = spsr::app(4).await;
    let wedge = spsr::wedge(&python, "stalled_tool").await;

    let healthy = spsr::tool_spec(30);
    let start = |tool: &'static str, sleep_s: f64| {
        let (kind, spec) = (python.clone(), healthy.clone());
        tokio::spawn(async move { spsr::call_ready(&kind, &spec, json!({"n": 5, "sleep_s": sleep_s}), &spsr::ctx(tool)).await })
    };
    let slow = start("slow_tool", 20.0);
    let fast = start("fast_tool", 0.5);

    let _subscriber = mcphost::pool_restart::spawn_subscriber(state.clone());
    spsr::breach(&state);
    let row = spsr::wait_row(&state).await;
    assert_eq!(row.outcome.as_str(), "ok", "row: {row:?}");

    let err = slow.await.unwrap().expect_err("the call that cannot finish inside the budget is cancelled");
    let KindError::Structured { code, data, .. } = &err else { panic!("got {err:?}") };
    assert_eq!(*code, "tool_timeout", "{err:?}");
    assert_eq!(data["heal_action_id"], row.id, "{data}");
    assert!(data["retry_after_ms"].as_u64().unwrap_or(0) > 0, "{data}");
    assert!(data["hint"].as_str().is_some_and(|h| h.contains("restarted")), "{data}");

    let ok = fast.await.unwrap().expect("a call that finishes inside the budget returns ok");
    assert_eq!(ok["n"], 10, "{ok}");
    let _ = wedge.call.await;
}
