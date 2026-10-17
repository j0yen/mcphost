//! PRD-mcphost-sandbox-pool-self-restart AC6 (P0): a call arriving while the
//! pool is `draining` waits (does not fail) and its result carries
//! `queued_ms >= 0`; when the drain outlasts `drain_budget_s + 5` the waiter
//! gets `sandbox_unavailable` with `retry_after_s` from `queue_wait_estimate_s`.

use crate::common;

use std::sync::Arc;
use std::time::{Duration, Instant};

use mcphost::kinds::python::PythonKind;
use mcphost::kinds::{CallCtx, Kind, KindError};
use mcphost::sandbox;
use serde_json::{Value, json};

fn spec() -> Value {
    json!({
        "source": "def main(args):\n    return {\"ok\": True}\n",
        "args_schema": {"type": "object"},
        "requirements": [],
    })
}

async fn call(kind: &PythonKind, ctx: &CallCtx) -> Result<Value, KindError> {
    let give_up = Instant::now() + Duration::from_secs(60);
    loop {
        match kind.call(&spec(), json!({}), ctx).await {
            Ok(v) if v.get("status").and_then(Value::as_str) == Some("building") && Instant::now() < give_up => {
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            other => return other,
        }
    }
}

#[tokio::test]
async fn a_call_during_the_drain_waits_and_carries_queued_ms_then_a_long_drain_is_sandbox_unavailable() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let data_dir = common::TempDataDir::new();
    let python = Arc::new(PythonKind::for_test_with_warm_pool(&data_dir.0, Duration::from_secs(60), 4, 8));
    let ctx = common::python_ctx(7, "spsr6", "waiter_tool");
    call(&python, &ctx).await.expect("prime: builds the environment");

    // A drain that ends inside the budget: the call waits, then succeeds.
    python.begin_drain_for_test(Duration::from_secs(30));
    let waiter = {
        let (python, ctx) = (python.clone(), common::python_ctx(7, "spsr6", "waiter_tool"));
        tokio::spawn(async move { call(&python, &ctx).await })
    };
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert!(!waiter.is_finished(), "a call during a drain waits, it does not fail");
    python.end_drain_for_test();
    let result = waiter.await.expect("join").expect("the waiter succeeds once the pool is ready");
    assert_eq!(result["ok"], json!(true));
    assert!(result["queued_ms"].as_u64().is_some_and(|ms| ms >= 300), "result: {result}");

    // A drain that outlasts drain_budget_s + 5: sandbox_unavailable.
    python.set_queue_wait_estimate_for_test(Some(7));
    python.begin_drain_for_test(Duration::ZERO);
    let err = call(&python, &ctx).await.expect_err("a drain past budget + 5 s is refused");
    let KindError::Structured { code, data, .. } = err else { panic!("expected a structured error, got {err:?}") };
    assert_eq!(code, "sandbox_unavailable");
    let estimate = python.queue_wait_estimate_s().expect("an estimate while draining");
    assert_eq!(data["retry_after_s"], json!(estimate.clamp(1, 30)), "data: {data}");
    assert!(estimate >= 7, "the estimate includes the base wait: {estimate}");
    python.end_drain_for_test();
}
