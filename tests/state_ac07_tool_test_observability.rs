//! PRD-mcphost-tenant-state
//! AC7 — Given a tool that reads two keys and writes one, When
//! `host.tool_test` runs it, Then the result carries `state: {reads: 2,
//! writes: 1, keys: [...]}` and `host.tool_logs` shows one write line with
//! the key.
//!
//! `host.tool_test` itself never appears in `host.tool_logs` -- that split
//! is pre-existing and covered elsewhere (`tooltest_ac12_log_marking.rs`'s
//! "test invocations are distinguishable from production calls", and the
//! `host.quickstart` note that a test call "counts toward neither
//! host.usage nor host.tool_logs, so it's safe to repeat while iterating").
//! So this test reads the AC's two clauses as two observability guarantees
//! requirement 5 adds, proven against the call path each one actually
//! belongs to: the `state` tally on `host.tool_test`'s own result (this
//! call, which also really touches the tenant's state store -- "test" only
//! means unbilled and unlogged, not simulated), and the `state_write` log
//! line format on a real published call to the same tool afterward.

use crate::common;
use common::{TestServer, extract_structured, poll_until_ready, python_kind_registry, signup};
use mcphost::sandbox;
use serde_json::json;
use std::time::Duration;

#[tokio::test]
async fn tool_test_result_carries_state_tally_and_a_real_call_logs_the_write() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs_dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (ns, key) = signup(&server.base_url, "State AC7 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    // Reads "k1" and "k2" (neither ever set -- default None), writes "k3".
    let spec = json!({
        "source": "import mcphost\ndef main(args):\n    a = mcphost.state.get(\"k1\", None)\n    b = mcphost.state.get(\"k2\", None)\n    mcphost.state.set(\"k3\", 1)\n    return {\"a\": a, \"b\": b}\n",
    });
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "reader_writer", "kind": "python", "spec": spec}),
        )
        .await
        .expect("publish ok");

    // host.tool_test: really runs the tool (cold start, same as any first
    // call), so poll for readiness the same way state_ac02_ac03 does.
    let tested = poll_until_ready(
        &client,
        "host.tool_test",
        json!({"name": "reader_writer", "args": {}}),
        Duration::from_secs(10),
    )
    .await
    .unwrap_or_else(|e| panic!("host.tool_test must succeed: {} {}", e.code, e.message));
    let structured = extract_structured(&tested);
    let state = &structured["state"];
    assert_eq!(state["reads"], json!(2), "two mcphost.state.get calls: {structured}");
    assert_eq!(state["writes"], json!(1), "one mcphost.state.set call: {structured}");
    let keys = state["keys"]
        .as_array()
        .unwrap_or_else(|| panic!("state.keys must be an array: {structured}"));
    for k in ["k1", "k2", "k3"] {
        assert!(
            keys.iter().any(|v| v.as_str() == Some(k)),
            "state.keys must name {k}: {structured}"
        );
    }

    // host.tool_test counts toward neither host.usage nor host.tool_logs
    // (pre-existing invariant) -- the bucket for this tool must still be
    // empty after the test call above.
    let logs_after_test = extract_structured(
        &client
            .tools_call("host.tool_logs", json!({"name": "reader_writer"}))
            .await
            .expect("host.tool_logs"),
    );
    let lines_after_test = logs_after_test["lines"].as_array().cloned().unwrap_or_default();
    assert!(
        lines_after_test.is_empty(),
        "host.tool_test must not write host.tool_logs lines: {logs_after_test}"
    );

    // A real call to the same tool (warm pool by now) does the same reads
    // and write; requirement 5's `host.tool_logs` clause is proven here.
    let qualified = format!("{ns}.reader_writer");
    client
        .tools_call(&qualified, json!({}))
        .await
        .expect("real call must succeed");

    let logs_after_real_call = extract_structured(
        &client
            .tools_call("host.tool_logs", json!({"name": "reader_writer"}))
            .await
            .expect("host.tool_logs"),
    );
    let lines = logs_after_real_call["lines"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let write_lines: Vec<&str> = lines
        .iter()
        .filter_map(|l| l.as_str())
        .filter(|l| l.starts_with("state_write"))
        .collect();
    assert_eq!(
        write_lines.len(),
        1,
        "exactly one state_write line for the one mcphost.state.set call: {lines:?}"
    );
    assert!(
        write_lines[0].contains("key=k3"),
        "the state_write line must carry the key: {}",
        write_lines[0]
    );
    assert!(
        write_lines[0].contains("bytes_delta="),
        "the state_write line must carry a byte delta: {}",
        write_lines[0]
    );
}
