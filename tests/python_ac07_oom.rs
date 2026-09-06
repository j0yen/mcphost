//! AC7 (P0) — Given a tool that allocates 2 GiB with `memory_mb` 256, When
//! called, Then the error is `tool_oom` and the host process's memory is
//! unchanged.
//!
//! "The host process's memory is unchanged" follows from the design (the
//! `RLIMIT_AS` limit `sandbox::run` applies in `pre_exec` binds only the
//! sandboxed child, never the host's own process) rather than being
//! re-measured here, the same scoping every other AC test in this suite
//! that names an OS-level property (AC6, AC10) uses.

mod common;
use common::{TestServer, poll_until_ready, python_kind_registry, signup};
use mcphost::sandbox;
use serde_json::json;
use std::time::Duration;

#[tokio::test]
async fn allocating_past_the_memory_limit_is_tool_oom() {
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
    let (ns, key) = signup(&server.base_url, "AC7 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    let spec = json!({
        "source": "def main(args):\n    x = bytearray(2 * 1024 * 1024 * 1024)\n    return {\"len\": len(x)}\n",
        "args_schema": {"type": "object"},
        "memory_mb": 256,
    });
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "hog", "kind": "python", "spec": spec}),
        )
        .await
        .expect("publish ok");

    let err = poll_until_ready(
        &client,
        &format!("{ns}.hog"),
        json!({}),
        Duration::from_secs(10),
    )
    .await
    .expect_err("allocating past the memory limit must not succeed");
    assert_eq!(err.error_code.as_deref(), Some("tool_oom"));
}
