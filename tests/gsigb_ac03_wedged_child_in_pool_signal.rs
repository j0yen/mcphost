//! PRD-mcphost-golden-signals-self-probe-b AC3 (P1): a pool child past 3x
//! its wall-clock timeout with no result, beside two healthy children, reads
//! `sandbox_pool.value == 0.333`; the wedged child's key is in the tracing
//! line and not in `/status.json`.
//!
//! Captured through `common::tracecap` (thread-keyed capture on the one
//! global subscriber; `#[tokio::test]` runs the tick on this thread).

use crate::common;

use std::sync::Arc;
use std::time::Duration;

use mcphost::kinds::python::PythonKind;

#[tokio::test]
async fn one_wedged_of_three_children_reads_a_third_with_the_key_only_in_the_log() {
    let (mut state, data_dir) = common::bare_app_state().await;
    let python = Arc::new(PythonKind::for_test_with_warm_pool(&data_dir.0, Duration::from_secs(60), 4, 8));
    state.kinds.register(python.clone());

    let timeout = Duration::from_millis(100);
    let _wedged = python.hold_test_flight("7/wedged_tool_zq", timeout, timeout * 4);
    let _healthy_a = python.hold_test_flight("7/healthy_a", timeout * 1000, Duration::ZERO);
    let _healthy_b = python.hold_test_flight("7/healthy_b", timeout * 1000, Duration::ZERO);

    // The signal needs two ticks ("after two ticks" below): the first is untimed
    // and uncaptured, the second runs under the capture.
    mcphost::statusfeed::tick_once(&state).await.expect("first tick");

    let buf = common::tracecap::capture_text();

    mcphost::statusfeed::tick_once(&state).await.expect("tick");

    let status = state.signals.status_json(mcphost::state::now_unix());
    let pool = &status["sandbox_pool"];
    let value = pool["value"].as_f64().expect("sandbox_pool value after two ticks");
    assert!((value - 0.333).abs() <= 0.01, "sandbox_pool: {pool}");
    assert!(!status.to_string().contains("wedged_tool_zq"), "key leaked into /status.json signals: {status}");

    let logged = buf.text();
    let line = logged
        .lines()
        .find(|l| l.contains("signal=sandbox_pool"))
        .unwrap_or_else(|| panic!("no sandbox_pool tracing line in:\n{logged}"));
    assert!(line.contains("wedged_tool_zq"), "line: {line}");
    assert!(!line.contains("healthy_a") && !line.contains("healthy_b"), "line: {line}");
}
