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

mod common;
use common::{TestServer, fake_interpreter_failing, python_kind_registry_with_selftest, signup};
use mcphost::sandbox;
use serde_json::json;

async fn healthz(base_url: &str) -> serde_json::Value {
    reqwest::get(format!("{base_url}/healthz"))
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

#[tokio::test]
async fn ready_sandbox_reports_ok_within_5s_of_start() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let data_dir = common::TempDataDir::new();
    let (kinds, py) = python_kind_registry_with_selftest(&data_dir.0, 300);

    let before = std::time::Instant::now();
    let status = py.run_startup_selftest().await;
    assert!(
        status.ready,
        "expected ready on a working sandbox, got: {status:?}"
    );
    assert_eq!(status.detail, format!("{}: ok", status.mechanism.as_str()));

    let server = TestServer::start_with_kinds(kinds).await;
    let body = healthz(&server.base_url).await;
    assert_eq!(body["sandbox_ready"], json!(true), "healthz: {body}");
    assert!(
        before.elapsed() < std::time::Duration::from_secs(5),
        "sandbox_checked_at must be within 5s of start"
    );
}
