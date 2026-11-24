//! PRD-mcphost-sandbox-pool-self-restart-v2 AC1: a warm child stopped with
//! SIGSTOP and a call whose arguments exceed the pipe buffer block
//! `call_inner` in its stdin write, before any deadline. The detector names
//! exactly that flight; the kill (through the call's own cancel slot) returns
//! the call within 1 s as `tool_timeout`, and healthy calls beside it finish
//! `ok`.

use crate::spsr;

use std::time::Duration;

use serde_json::json;

const WEDGE_FACTOR: f64 = 0.08;

#[tokio::test]
async fn spsr_ac01() {
    if mcphost::sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", mcphost::sandbox::USERNS_SKIP_MARKER);
        return;
    }
    // The stalled tool's flight timeout is 1 s + 2; the healthy ones' is 30 s + 2.
    let stalled_spec = spsr::tool_spec(1);
    let healthy_spec = spsr::tool_spec(30);
    let kind = spsr::kind_with_warm_child("ac01", "stalled_tool", &stalled_spec).await;
    spsr::stop_warm_children(&kind);

    let stalled = {
        let kind = kind.clone();
        let spec = stalled_spec.clone();
        tokio::spawn(async move { spsr::call_ready(&kind, &spec, spsr::big_args(), &spsr::ctx("stalled_tool")).await })
    };
    let healthy: Vec<_> = ["healthy_a", "healthy_b"]
        .into_iter()
        .map(|tool| {
            let kind = kind.clone();
            let spec = healthy_spec.clone();
            tokio::spawn(async move {
                spsr::call_ready(&kind, &spec, json!({"n": 4, "sleep_s": 1.5}), &spsr::ctx(tool)).await
            })
        })
        .collect();

    tokio::time::sleep(Duration::from_millis(700)).await;
    let signals = mcphost::kinds::Kind::pool_signals(kind.as_ref(), WEDGE_FACTOR).expect("python kind has a pool");
    assert_eq!(signals.wedged_keys, vec![spsr::STALLED_KEY.to_string()], "signals: {signals:?}");
    assert_eq!(signals.wedged, 1);
    assert!(!stalled.is_finished(), "the stalled call has no deadline of its own while its write is blocked");

    assert_eq!(kind.kill_wedged_flights(WEDGE_FACTOR), vec![spsr::STALLED_KEY.to_string()]);
    let joined = tokio::time::timeout(Duration::from_secs(1), stalled).await;
    let err = joined.expect("the killed call returns within 1 s").unwrap().expect_err("the killed call fails");
    assert!(spsr::is_tool_timeout(&err), "got {err:?}");

    for call in healthy {
        let ok = call.await.unwrap().expect("a healthy call beside the wedge returns ok");
        assert_eq!(ok["n"], 8, "{ok}");
    }
}
