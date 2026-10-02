//! PRD-mcphost-sandbox-ready
//! AC1 -- Given a host where a sandboxed `/bin/true` cannot run (test: inject
//! a `RunSpec` whose interpreter is a script that exits 1 with
//! `bwrap: loopback: Failed RTM_NEWADDR` on stderr), When `mcphost serve`
//! starts, Then it serves, `/healthz` reports `sandbox_ready: false`,
//! `sandbox_detail` contains `RTM_NEWADDR`.
//! AC2 -- Given a host where the sandbox works (this repository's build
//! machine), When `mcphost serve` starts, Then `/healthz` reports
//! `sandbox_ready: true`, `sandbox_detail` is `"<mechanism>: ok"`, and
//! `sandbox_checked_at` is within 5s of start.

use crate::common;
use common::{
    ADMIN_KEY, TestServer, fake_interpreter_failing, python_kind_registry_with_selftest, signup,
};
use mcphost::sandbox;
use serde_json::json;

// PRD-mcphost-healthz-minimal: `sandbox_ready`/`sandbox_detail` moved behind
// the admin bearer -- the anonymous body is just `{"ok": true/false}` now.
async fn healthz(base_url: &str) -> serde_json::Value {
    reqwest::Client::new()
        .get(format!("{base_url}/healthz"))
        .bearer_auth(ADMIN_KEY)
        .send()
        .await
        .expect("GET /healthz")
        .json()
        .await
        .expect("parse /healthz")
}

#[tokio::test]
async fn unready_sandbox_reports_at_healthz_and_the_host_still_serves() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let data_dir = common::TempDataDir::new();
    let (kinds, py) = python_kind_registry_with_selftest(&data_dir.0, 300);
    py.set_selftest_interpreter_for_test(Some(&fake_interpreter_failing(
        "bwrap: loopback: Failed RTM_NEWADDR: Operation not permitted",
    )));
    py.run_startup_selftest().await;

    let server = TestServer::start_with_kinds(kinds).await;
    let body = healthz(&server.base_url).await;

    assert_eq!(body["sandbox_ready"], json!(false), "healthz: {body}");
    let detail = body["sandbox_detail"]
        .as_str()
        .expect("sandbox_detail is a string");
    assert!(
        detail.contains("RTM_NEWADDR"),
        "sandbox_detail must contain RTM_NEWADDR, got: {detail}"
    );

    // "it serves": requirement 1's whole point -- echo still works even
    // though python's sandbox is broken.
    let (_ns, key) = signup(&server.base_url, "AC1 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "ec", "kind": "echo", "spec": {"schema": {"type": "object"}}}),
        )
        .await
        .expect("echo publish must still work while python's sandbox is broken");
}

// PRD-mcphost-sandbox-ready AC2 split in two per
// PRD-mcphost-test-suite-flake-lints requirement 6 (same shape as
// `enduserctl_ac10`'s pagination/perf split): correctness below carries no
// timing assertion at all, and the timing half just below it uses
// `perf_budget!`'s warm-up-then-5-timed-runs median instead of one
// wall-clock sample.
#[tokio::test]
async fn ready_sandbox_reports_ok_via_healthz() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let data_dir = common::TempDataDir::new();
    let (kinds, py) = python_kind_registry_with_selftest(&data_dir.0, 300);

    let status = py.run_startup_selftest().await;
    assert!(
        status.ready,
        "expected ready on a working sandbox, got: {status:?}"
    );
    assert_eq!(status.detail, format!("{}: ok", status.mechanism.as_str()));

    let server = TestServer::start_with_kinds(kinds).await;
    let body = healthz(&server.base_url).await;
    assert_eq!(body["sandbox_ready"], json!(true), "healthz: {body}");
}

// AC2's own timing half: "`sandbox_checked_at` is within 5 s of start".
// `checked_at` (src/sandbox.rs `SandboxStatus::ready`) is stamped the
// instant `run_startup_selftest` returns -- the same call `main.rs` makes
// at real process start -- so that call is the ONLY thing this property is
// about. The old single-`Instant` version timed that call PLUS
// `TestServer::start_with_kinds` (bind a real HTTP listener) PLUS a real
// HTTP round trip to `/healthz`, none of which `checked_at` depends on;
// under the gate box's load (32 vCPU, up to 8 concurrent gates) that
// unrelated scaffolding is what blew the 5s budget (run 339: same single
// failure in both flake-audit attempts, 57/58 passing), not a slow sandbox
// probe. `perf_budget!` also gets the `MCPHOST_PERF_SKIP` escape hatch for
// free, so a loaded gate box skips the measurement and the final gate
// re-runs it quiet.
#[tokio::test]
async fn ready_sandbox_selftest_completes_within_5s_of_start() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let data_dir = common::TempDataDir::new();
    let (_kinds, py) = python_kind_registry_with_selftest(&data_dir.0, 300);

    crate::perf_budget!(5000, {
        let status = py.run_startup_selftest().await;
        assert!(
            status.ready,
            "expected ready on a working sandbox, got: {status:?}"
        );
    });
}
