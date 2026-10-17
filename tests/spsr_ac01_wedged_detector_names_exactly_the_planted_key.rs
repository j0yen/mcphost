//! PRD-mcphost-sandbox-pool-self-restart AC1 (P0): a warm pool with one child
//! planted past 3x `wall_clock_timeout` with no result and two children
//! answering within budget: `WarmPool::wedged(now)` names exactly the planted
//! key, and the two healthy calls complete `ok` after the restart executes.

use crate::common;

use std::sync::Arc;
use std::time::{Duration, Instant};

use mcphost::kinds::python::PythonKind;
use mcphost::kinds::{CallCtx, Kind};
use mcphost::sandbox;
use serde_json::{Value, json};

fn healthy_spec() -> Value {
    json!({
        "source": "import time\ndef main(args):\n    time.sleep(1)\n    return {\"ok\": True}\n",
        "args_schema": {"type": "object"},
        "requirements": [],
    })
}

async fn call_until_built(kind: &PythonKind, spec: &Value, ctx: &CallCtx) -> Result<Value, mcphost::kinds::KindError> {
    let give_up = Instant::now() + Duration::from_secs(60);
    loop {
        match kind.call(spec, json!({}), ctx).await {
            Ok(v) if v.get("status").and_then(Value::as_str) == Some("building") && Instant::now() < give_up => {
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            other => return other,
        }
    }
}

#[tokio::test]
async fn detector_names_exactly_the_planted_key_and_healthy_calls_finish_ok() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let (mut state, data_dir) = common::bare_app_state().await;
    let python = Arc::new(PythonKind::for_test_with_warm_pool(&data_dir.0, Duration::from_secs(60), 4, 8));
    state.kinds.register(python.clone());

    let timeout = Duration::from_millis(100);
    let mut wedged = common::WedgedChild::spawn(b"");
    let _planted = python.hold_test_flight_with_cancel("7/wedged_tool", timeout, timeout * 4, Some(wedged.slot.clone()));
    assert_eq!(python.wedged(Instant::now()), vec![(7, "wedged_tool".to_string())]);

    let spec = healthy_spec();
    let mut calls = Vec::new();
    for name in ["healthy_a", "healthy_b"] {
        let (python, spec) = (python.clone(), spec.clone());
        calls.push(tokio::spawn(async move {
            call_until_built(&python, &spec, &common::python_ctx(7, &format!("spsr1{name}"), name)).await
        }));
    }
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(
        python.wedged(Instant::now()),
        vec![(7, "wedged_tool".to_string())],
        "the two calls still inside their budget are not wedged"
    );

    let row = mcphost::pool_restart::run(&state, &common::pool_breach()).await.expect("restart").expect("python kind");
    assert_eq!(row.outcome.as_str(), "ok");
    assert!(wedged.is_dead().await, "the wedged child was killed");

    for call in calls {
        let result = call.await.expect("join").expect("healthy call completes ok after the restart");
        assert_eq!(result["ok"], json!(true));
    }
}
