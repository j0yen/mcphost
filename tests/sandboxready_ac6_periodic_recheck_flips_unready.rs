//! PRD-mcphost-sandbox-ready
//! AC6 -- Given `sandbox_ready: true` and a call that then breaks the
//! sandbox (test: injected interpreter replaced by a failing one), When the
//! periodic recheck runs (a 1s interval in this test), Then within 3s
//! `/healthz` reports `sandbox_ready: false` and the warm pool is empty.

mod common;
use common::{fake_interpreter_failing, poll_until_ready, signup};
use mcphost::kinds::python::PythonKind;
use mcphost::sandbox;
use serde_json::json;
use std::sync::Arc;
use std::time::{Duration, Instant};

async fn healthz(base_url: &str) -> serde_json::Value {
    reqwest::get(format!("{base_url}/healthz"))
        .await
        .expect("GET /healthz")
        .json()
        .await
        .expect("parse /healthz")
}

#[tokio::test]
async fn periodic_recheck_flips_ready_to_unready_and_empties_the_warm_pool() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let data_dir = common::TempDataDir::new();
    // recheck_secs = 1 applies to the interval used *while ready* too --
    // see `SandboxSelftest::interval`'s doc comment for why this crate's
    // tests use a per-instance override rather than mutating
    // $MCPHOST_SANDBOX_RECHECK_SECS process-wide.
    let py = Arc::new(PythonKind::for_test_with_selftest(&data_dir.0, 1));
    let mut kinds = mcphost::kinds::KindRegistry::with_builtin();
    kinds.register(py.clone());

    let status = py.run_startup_selftest().await;
    assert!(status.ready, "must start ready: {status:?}");

    let server = common::TestServer::start_with_kinds(kinds).await;
    let (ns, key) = signup(&server.base_url, "AC6 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);
    client
        .tools_call(
            "host.tool_publish",
            json!({
                "name": "warm_me",
                "kind": "python",
                "spec": {"source": "def main(args):\n    return {}\n"},
            }),
        )
        .await
        .expect("publish");

    // A successful cold call seeds the warm pool (best-effort, async) --
    // poll briefly for the pool to actually gain an entry before breaking
    // the sandbox, so the eviction this test proves is meaningful.
    poll_until_ready(
        &client,
        &format!("{ns}.warm_me"),
        json!({}),
        Duration::from_secs(30),
    )
    .await
    .unwrap_or_else(|e| panic!("first call must succeed: {} {}", e.code, e.message));
    let seeded = Instant::now();
    loop {
        if py.warm_metrics().2 > 0 || seeded.elapsed() > Duration::from_secs(5) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(
        py.warm_metrics().2 > 0,
        "expected the warm pool to have gained an entry before the flip"
    );

    // Break the sandbox and let the (1s-interval) periodic recheck find it.
    py.set_selftest_interpreter_for_test(Some(&fake_interpreter_failing(
        "bwrap: loopback: Failed RTM_NEWADDR: Operation not permitted",
    )));

    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        let body = healthz(&server.base_url).await;
        if body["sandbox_ready"] == json!(false) {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "sandbox_ready must flip to false within 3s of the periodic recheck"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(
        py.warm_metrics().2,
        0,
        "the warm pool must be empty after a ready->unready flip"
    );
}
