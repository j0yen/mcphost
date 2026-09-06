//! AC14 (P0) — Given a tenant over its CPU budget for the hour, When it
//! calls any `python` tool, Then `rate_limited` with `retry_after_s` is
//! returned and no process starts.
//!
//! Uses `python_kind_registry_with_cpu_budget_ms(_, 1)` -- a 1ms/hour
//! budget any real call immediately exceeds -- rather than 600 real
//! CPU-seconds of calls (see `kinds::python`'s module docs on this scoped
//! decision). "No process starts" is proven by asserting the second call
//! returns *immediately* (well under the tool's own sleep duration), which
//! could only happen if the budget check short-circuited before
//! `sandbox::run` was ever invoked.

mod common;
use common::{TestServer, poll_until_ready, python_kind_registry_with_cpu_budget_ms, signup};
use mcphost::sandbox;
use serde_json::json;
use std::time::{Duration, Instant};

#[tokio::test]
async fn a_tenant_over_budget_is_rate_limited_without_starting_a_process() {
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
    let server =
        TestServer::start_with_kinds(python_kind_registry_with_cpu_budget_ms(&envs_dir.0, 1)).await;
    let (ns, key) = signup(&server.base_url, "AC14 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    let spec = json!({
        "source": "import time\ndef main(args):\n    time.sleep(2)\n    return {\"ok\": True}\n",
        "args_schema": {"type": "object"},
    });
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "budgeted", "kind": "python", "spec": spec}),
        )
        .await
        .expect("publish ok");

    // The first call: any nonzero CPU usage it reports pushes the tenant
    // over a 1ms/hour budget for every call after it.
    let qualified = format!("{ns}.budgeted");
    let _ = poll_until_ready(&client, &qualified, json!({}), Duration::from_secs(10)).await;

    let started = Instant::now();
    let err = client
        .tools_call(&qualified, json!({}))
        .await
        .expect_err("a call over budget must be refused");
    let elapsed = started.elapsed();
    assert_eq!(err.error_code.as_deref(), Some("rate_limited"));
    assert!(
        err.data["retry_after_s"].as_u64().is_some_and(|s| s > 0),
        "rate_limited must carry a positive retry_after_s, got: {:?}",
        err.data
    );
    assert!(
        elapsed < Duration::from_millis(500),
        "a budget-refused call must return immediately, without starting the (2s-sleeping) \
         sandboxed process; took {elapsed:?}"
    );
}
