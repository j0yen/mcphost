//! AC1 (P0) — Given a tenant, When it publishes a `python` tool with no
//! requirements and calls it within 5 s, Then the call returns `main`'s
//! result and the `calls` row records cpu and memory.

mod common;
use common::{TestServer, extract_structured, poll_until_ready, python_kind_registry, signup};
use mcphost::sandbox;
use serde_json::json;
use std::time::{Duration, Instant};

#[tokio::test]
async fn no_dependency_tool_runs_within_five_seconds_and_meters_usage() {
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
    let (ns, key) = signup(&server.base_url, "AC1 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    let spec = json!({
        "source": "def main(args):\n    return {\"sum\": args[\"a\"] + args[\"b\"]}\n",
        "args_schema": {
            "type": "object",
            "properties": {"a": {"type": "integer"}, "b": {"type": "integer"}},
            "required": ["a", "b"],
        },
    });
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "adder", "kind": "python", "spec": spec}),
        )
        .await
        .expect("publish ok");

    let started = Instant::now();
    let result = poll_until_ready(
        &client,
        &format!("{ns}.adder"),
        json!({"a": 2, "b": 3}),
        Duration::from_secs(5),
    )
    .await
    .unwrap_or_else(|e| panic!("call must succeed within 5s: {} {}", e.code, e.message));
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "publish-to-callable must land within 5s for a no-dependency tool"
    );
    assert_eq!(extract_structured(&result)["sum"], json!(5));

    let tenant = server
        .state
        .db
        .find_tenant_by_namespace(ns)
        .await
        .expect("db read")
        .expect("tenant exists");
    let usage = server
        .state
        .db
        .last_call_usage(tenant.id, "adder".to_string())
        .await
        .expect("db read")
        .expect("a calls row must exist for the successful call");
    assert!(
        usage.0.is_some(),
        "the calls row must record cpu_ms, got {usage:?}"
    );
    assert!(
        usage.1.is_some(),
        "the calls row must record peak_rss_kb, got {usage:?}"
    );
}
