//! PRD-mcphost-dry-run-side-effects
//! AC4 — Given `host.tool_run(name, args, test: true)` on a python tool
//! that writes state, When it completes, Then stdout/stderr/exit code are
//! returned as today and `dry_run.writes` lists the state write while
//! `host.state.get` returns the prior value.
//!
//! Before this PRD, `host.tool_run` had no notion of `test` at all (no
//! argument read, `ctx.test_mode` hardcoded `false`,
//! `TenantStateBridge`/`TenantTableBridge` wired unconditionally) --
//! `test: true` was simply ignored and the write landed for real.

use crate::common;
use common::{McpClient, TestServer, extract_structured, python_kind_registry, signup};
use mcphost::sandbox;
use serde_json::json;
use std::time::Duration;

#[tokio::test]
async fn tool_run_test_true_rolls_back_state_write_and_keeps_stdout() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs_dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (_ns, key) = signup(&server.base_url, "Dryrun AC4 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    // Seed a prior value so the test can prove it's untouched afterward.
    client
        .tools_call("host.state.set", json!({"key": "counter", "value": 1}))
        .await
        .expect("seed state ok");

    let source = "import sys\nimport mcphost\ndef main(args):\n    mcphost.state.set(\"counter\", 999)\n    print(\"hello from tool_run\")\n    return {\"ok\": True}\n";
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "writer", "kind": "python", "spec": {"source": source}}),
        )
        .await
        .expect("publish ok");

    // `host.tool_run` isn't wrapped by `poll_until_ready` in the existing
    // test suite (it's a cold-start-tolerant debug path of its own, same
    // rationale `warmpool_ac06_ac07_tool_run.rs` already exercises), but
    // the first call into a never-built env still needs a moment -- retry
    // like `poll_until_ready` does for `tools/call`/`host.tool_test`.
    let mut last_err = None;
    let mut result = None;
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while std::time::Instant::now() < deadline {
        match client
            .tools_call("host.tool_run", json!({"name": "writer", "args": {}, "test": true}))
            .await
        {
            Ok(v) => {
                result = Some(v);
                break;
            }
            Err(e) => {
                last_err = Some(e);
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
        }
    }
    let result = result.unwrap_or_else(|| {
        let e = last_err.expect("some error recorded");
        panic!("host.tool_run(test: true) must eventually succeed: {} {}", e.code, e.message)
    });
    let structured = extract_structured(&result);

    assert!(
        structured["stdout"]
            .as_str()
            .unwrap_or_default()
            .contains("hello from tool_run"),
        "stdout must be returned exactly as a real host.tool_run: {structured}"
    );

    let dry_run = &structured["dry_run"];
    assert_eq!(dry_run["delivered"], json!(false), "{structured}");
    assert_eq!(dry_run["rolled_back"], json!(true), "{structured}");
    let writes = dry_run["writes"].as_array().unwrap_or_else(|| panic!("writes array: {structured}"));
    assert_eq!(writes.len(), 1, "{structured}");
    assert_eq!(writes[0]["store"], json!("state"));
    assert_eq!(writes[0]["op"], json!("set"));

    let after = extract_structured(
        &client
            .tools_call("host.state.get", json!({"key": "counter"}))
            .await
            .expect("host.state.get ok"),
    );
    assert_eq!(
        after["value"],
        json!(1),
        "host.tool_run(test: true)'s write must never have persisted: {after}"
    );
}
