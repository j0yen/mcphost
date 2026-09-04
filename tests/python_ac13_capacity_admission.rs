//! AC13 (P0) — Given 21 concurrent calls with the default limit of 20,
//! When the 21st arrives, Then it returns `capacity` immediately and the
//! other 20 complete.
//!
//! Uses `python_kind_registry_with_concurrency(_, 2)` rather than 21 real
//! sandboxed processes in flight (AC10/AC11's `http`-kind tests scale down
//! the same way for their own 500/21-way load tests) -- the property under
//! test, an exact concurrency ceiling that refuses the Nth-plus-one caller
//! immediately rather than queuing it, doesn't depend on which N.

mod common;
use common::{TestServer, poll_until_ready, python_kind_registry_with_concurrency, signup};
use mcphost::sandbox;
use serde_json::json;
use std::time::Duration;

const CONCURRENCY_LIMIT: usize = 2;

#[tokio::test]
async fn the_call_over_the_limit_gets_capacity_immediately() {
    // Requirement 8/9: this test builds and runs a real python-kind tool
    // via the sandbox, which needs unprivileged user namespaces. Not
    // guaranteed on GitHub's hosted runners, so skip cleanly in CI (and
    // fail loudly, not skip, anywhere else -- see
    // require_user_namespaces_or_ci_skip's doc comment) rather than fail
    // with "the tool's environment failed to build" -- same pattern as the
    // sandbox-dependent unit tests in src/kinds/python.rs and
    // src/sandbox.rs, and as tests/ac17_kind_conformance.rs.
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("skipped: no user namespaces (CI)");
        return;
    }
    let envs_dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry_with_concurrency(
        &envs_dir.0,
        CONCURRENCY_LIMIT,
    ))
    .await;
    let (ns, key) = signup(&server.base_url, "AC13 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    let spec = json!({
        "source": "import time\ndef main(args):\n    time.sleep(0.8)\n    return {\"ok\": True}\n",
        "args_schema": {"type": "object"},
    });
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "slow", "kind": "python", "spec": spec}),
        )
        .await
        .expect("publish ok");

    // Warm the build so the concurrent wave below all hit the real
    // (sandboxed, slow) call path together, not a `tool_building` race.
    let _ = poll_until_ready(
        &client,
        &format!("{ns}.slow"),
        json!({}),
        Duration::from_secs(5),
    )
    .await;

    let qualified = format!("{ns}.slow");
    let mut handles = Vec::with_capacity(CONCURRENCY_LIMIT + 1);
    for _ in 0..CONCURRENCY_LIMIT + 1 {
        let c = common::McpClient::with_bearer(&server.base_url, &key);
        let q = qualified.clone();
        handles.push(tokio::spawn(
            async move { c.tools_call(&q, json!({})).await },
        ));
    }

    let mut ok_count = 0;
    let mut capacity_count = 0;
    for handle in handles {
        match handle.await.expect("task must not panic") {
            Ok(_) => ok_count += 1,
            Err(e) if e.error_code.as_deref() == Some("capacity") => capacity_count += 1,
            Err(e) => panic!("unexpected error: {} {}", e.code, e.message),
        }
    }

    assert_eq!(
        capacity_count, 1,
        "exactly one caller over the concurrency limit must get `capacity`"
    );
    assert_eq!(
        ok_count, CONCURRENCY_LIMIT,
        "every call within the limit must complete"
    );
}
