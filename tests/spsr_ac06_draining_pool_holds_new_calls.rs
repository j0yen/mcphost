//! PRD-mcphost-sandbox-pool-self-restart-v2 AC6: a new call against a
//! `draining` pool waits and completes `ok` after the drain; with `draining`
//! held past `drain_budget_s + 5` (a test seam) the waiter gets
//! `sandbox_unavailable` with `retry_after_s` from `queue_wait_estimate_s`.

use crate::spsr;

use std::time::Duration;

use mcphost::kinds::KindError;
use serde_json::json;

#[tokio::test]
async fn spsr_ac06() {
    if mcphost::sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", mcphost::sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let spec = spsr::tool_spec(30);
    let kind = spsr::kind_with_warm_child("ac06", "waiting_tool", &spec).await;

    // A drain that ends: the call waits, then completes ok.
    let hold = kind.hold_drain_for_test(Duration::from_secs(30));
    let waiter = {
        let (kind, spec) = (kind.clone(), spec.clone());
        tokio::spawn(async move { spsr::call_ready(&kind, &spec, json!({"n": 3}), &spsr::ctx("waiting_tool")).await })
    };
    tokio::time::sleep(Duration::from_millis(800)).await;
    assert!(!waiter.is_finished(), "the call waits while the pool drains");
    let pool = mcphost::kinds::Kind::pool_signals(kind.as_ref(), spsr::RIG_WEDGE_FACTOR).unwrap();
    assert!(pool.draining, "{pool:?}");
    assert_eq!(pool.in_flight_count, 0, "a waiting call is not in flight: {pool:?}");
    drop(hold);
    let ok = tokio::time::timeout(Duration::from_secs(20), waiter)
        .await
        .expect("the call completes after the drain")
        .unwrap()
        .expect("ok after the drain");
    assert_eq!(ok["n"], 6, "{ok}");

    // A drain held past `drain_budget_s + 5`: the waiter is refused.
    kind.set_queue_wait_estimate_for_test(Some(7));
    let _held = kind.hold_drain_for_test(Duration::from_secs(1));
    let refused = tokio::time::timeout(
        Duration::from_secs(30),
        spsr::call_ready(&kind, &spec, json!({"n": 1}), &spsr::ctx("waiting_tool")),
    )
    .await
    .expect("the waiter is bounded at the drain budget + 5 s");
    let err = refused.expect_err("sandbox_unavailable past the bound");
    let KindError::Structured { code, data, .. } = &err else { panic!("got {err:?}") };
    assert_eq!(*code, "sandbox_unavailable", "{err:?}");
    assert_eq!(data["retry_after_s"], 7, "{data}");
}
