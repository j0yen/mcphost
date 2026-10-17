//! PRD-mcphost-sandbox-pool-self-restart AC3 (P0): a healthy call that cannot
//! finish inside `drain_budget_s` fails with `tool_timeout` whose `data`
//! carries `heal_action_id` (the ledger row id) and `retry_after_ms > 0`; a
//! call that finishes inside the budget returns `ok`.

use crate::common;

use std::sync::Arc;
use std::time::{Duration, Instant};

use mcphost::kinds::python::PythonKind;
use mcphost::kinds::{CallCtx, Kind, KindError};
use mcphost::sandbox;
use serde_json::{Value, json};

fn spec() -> Value {
    json!({
        "source": "import time\ndef main(args):\n    time.sleep(args.get(\"s\", 0))\n    return {\"ok\": True}\n",
        "args_schema": {"type": "object"},
        "requirements": [],
    })
}

async fn call(kind: &PythonKind, ctx: &CallCtx, secs: u64) -> Result<Value, KindError> {
    let give_up = Instant::now() + Duration::from_secs(60);
    loop {
        match kind.call(&spec(), json!({"s": secs}), ctx).await {
            Ok(v) if v.get("status").and_then(Value::as_str) == Some("building") && Instant::now() < give_up => {
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            other => return other,
        }
    }
}

#[tokio::test]
async fn a_call_outliving_the_drain_budget_times_out_with_the_row_id_and_a_quick_one_is_ok() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let (mut state, data_dir) = common::bare_app_state().await;
    state.signals = common::signals_with_drain_budget(3);
    let python = Arc::new(PythonKind::for_test_with_warm_pool(&data_dir.0, Duration::from_secs(60), 4, 8));
    state.kinds.register(python.clone());

    let slow_ctx = common::python_ctx(7, "spsr3slow", "slow_tool");
    let quick_ctx = common::python_ctx(7, "spsr3quick", "quick_tool");
    // Builds both environments so the timed calls below start warm.
    call(&python, &slow_ctx, 0).await.expect("prime slow");
    call(&python, &quick_ctx, 0).await.expect("prime quick");

    let slow = {
        let python = python.clone();
        tokio::spawn(async move { call(&python, &slow_ctx, 15).await })
    };
    let quick = {
        let python = python.clone();
        tokio::spawn(async move { call(&python, &quick_ctx, 1).await })
    };
    tokio::time::sleep(Duration::from_millis(300)).await;

    let row = mcphost::pool_restart::run(&state, &common::pool_breach()).await.expect("restart").expect("python kind");
    assert_eq!(row.outcome.as_str(), "ok");

    assert_eq!(quick.await.expect("join").expect("a call inside the budget returns ok")["ok"], json!(true));
    let err = slow.await.expect("join").expect_err("a call past the budget fails");
    let KindError::Structured { code, data, .. } = err else { panic!("expected a structured error, got {err:?}") };
    assert_eq!(code, "tool_timeout");
    assert_eq!(data["heal_action_id"], json!(row.id), "data: {data}");
    assert!(data["retry_after_ms"].as_u64().unwrap_or(0) > 0, "data: {data}");
}
