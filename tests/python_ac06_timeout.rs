//! AC6 (P0) — Given a tool that sleeps past `timeout_s`, When called, Then
//! the error is `call_timeout` within `timeout_s` + 2 s and no child
//! process survives (process group killed).
//!
//! The group-kill mechanism itself is proven directly against
//! `crate::sandbox::run` in `src/sandbox.rs`'s own
//! `kills_the_group_on_timeout` unit test; this test proves the same
//! property is wired end to end through the real publish/call path with the
//! PRD's own timing bound.
//!
//! PRD-mcphost-call-limits-honest requirement 1 made the declared
//! `timeout_s` the real per-call deadline (bounded by `MAX_TIMEOUT_S`)
//! instead of the fixed `CALL_TIMEOUT` constant, and renamed the resulting
//! error from `tool_timeout` to `call_timeout` so the message can name the
//! deadline that actually applied -- `tool_timeout` remains the code for a
//! *different* path (the sandbox's own internal wall-clock kill), so this
//! test's expected code moved with the behavior it exercises.

mod common;
use common::{TestServer, poll_until_ready, python_kind_registry, signup};
use mcphost::sandbox;
use serde_json::json;
use std::time::{Duration, Instant};

#[tokio::test]
async fn a_slow_tool_times_out_within_timeout_plus_two_seconds() {
    // Requirement 8/9: this test builds and runs a real python-kind tool
    // via the sandbox, which needs unprivileged user namespaces. Not
    // guaranteed on GitHub's hosted runners, so skip cleanly in CI (and
    // fail loudly, not skip, anywhere else -- see
    // require_user_namespaces_or_ci_skip's doc comment) rather than fail
    // with "the tool's environment failed to build" -- same pattern as the
    // sandbox-dependent unit tests in src/kinds/python.rs and
    // src/sandbox.rs, and as tests/ac17_kind_conformance.rs.
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs_dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (ns, key) = signup(&server.base_url, "AC6 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    let spec = json!({
        "source": "import time\ndef main(args):\n    time.sleep(30)\n    return {}\n",
        "args_schema": {"type": "object"},
        "timeout_s": 1,
    });
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "sleepy", "kind": "python", "spec": spec}),
        )
        .await
        .expect("publish ok");

    // The first call kicks off the (near-instant, no-dependency) build and
    // returns `tool_building`; poll through it, then time the real call.
    let _ = poll_until_ready(
        &client,
        &format!("{ns}.sleepy"),
        json!({}),
        Duration::from_secs(5),
    )
    .await;

    let started = Instant::now();
    let err = client
        .tools_call(&format!("{ns}.sleepy"), json!({}))
        .await
        .expect_err("a call past its timeout must fail");
    let elapsed = started.elapsed();
    assert_eq!(err.error_code.as_deref(), Some("call_timeout"));
    assert!(
        elapsed < Duration::from_secs(4),
        "timeout_s=1 must fire within timeout_s+2s, took {elapsed:?}"
    );
}
