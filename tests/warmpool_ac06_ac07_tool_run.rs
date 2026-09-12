//! PRD-mcphost-code-tools-warm-pool
//!
//! AC6 (P0) — Given `host.tool_run` on a tool that prints and raises, When
//! called, Then the result carries full stdout and stderr up to 64 KiB
//! each, the exit code, and no `calls` row is written.
//!
//! AC7 (P0) — Given 31 `host.tool_run` calls in a minute from one tenant,
//! When the 31st arrives, Then it returns `rate_limited`.
//!
//! End-to-end through the real HTTP/JSON-RPC dispatch path (`handler.rs`'s
//! new `host.tool_run` RPC and `state::ToolRunLimiter`), same pattern as
//! every other `python_ac*.rs` test: a real `mcphost` server, a real
//! bwrap-sandboxed tool.

use crate::common;
use common::{TestServer, extract_structured, poll_until_ready, python_kind_registry, signup};
use mcphost::sandbox;
use serde_json::{Value, json};
use std::time::Duration;

#[tokio::test]
async fn tool_run_returns_full_output_with_no_calls_row_then_rate_limits_the_31st() {
    // Requirement 8/9: real sandboxed tool, needs unprivileged user
    // namespaces -- see the identical guard in every other python_ac*.rs
    // test / require_user_namespaces_or_ci_skip's own doc comment.
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs_dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (ns, key) = signup(&server.base_url, "Warmpool AC6/AC7 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    let spec = json!({
        "source": "def main(args):\n    print(\"hello from tool_run\")\n    raise ValueError(\"boom\")\n",
        "args_schema": {"type": "object"},
        "requirements": [],
    });
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "printer", "kind": "python", "spec": spec}),
        )
        .await
        .expect("publish ok");

    // Warm the (no-dependency) build via an ordinary call before timing
    // `host.tool_run` itself -- same `poll_until_ready` pattern every other
    // python_ac*.rs test uses. This ordinary call (and its retries) DOES
    // write `calls` rows, which is exactly why AC6 is checked as a *delta*
    // in `host.usage`, not an absolute zero, below.
    let _ = poll_until_ready(
        &client,
        &format!("{ns}.printer"),
        json!({}),
        Duration::from_secs(5),
    )
    .await;

    let usage_calls = |result: &serde_json::Value| -> i64 {
        extract_structured(result)["calls"].as_i64().unwrap_or(-1)
    };
    let before = client.tools_call("host.usage", json!({})).await.expect("usage before");
    let calls_before = usage_calls(&before);

    let run_result = client
        .tools_call("host.tool_run", json!({"name": "printer", "args": {}}))
        .await
        .expect("host.tool_run RPC must succeed even though the tool itself raised");
    let structured = extract_structured(&run_result);

    let stdout = structured["stdout"].as_str().expect("stdout field");
    let stderr = structured["stderr"].as_str().expect("stderr field");
    assert!(
        stdout.contains("hello from tool_run"),
        "stdout must carry the tool's own print, got: {stdout:?}"
    );
    assert!(
        stderr.contains("ValueError") && stderr.contains("boom"),
        "stderr must carry the traceback/message, got: {stderr:?}"
    );
    assert_eq!(structured["exit_code"], json!(1), "a raised exception is exit_code 1");
    assert!(
        structured["duration_ms"].is_number(),
        "duration_ms must be present"
    );
    assert_eq!(structured["result"], json!(Value::Null), "a raised call has no result");

    let after = client.tools_call("host.usage", json!({})).await.expect("usage after");
    let calls_after = usage_calls(&after);
    assert_eq!(
        calls_before, calls_after,
        "host.tool_run must write no `calls` row"
    );

    // AC7: 29 more calls (30 total including the one above) must all
    // succeed; the 31st must be rate_limited.
    for i in 0..29 {
        client
            .tools_call("host.tool_run", json!({"name": "printer", "args": {}}))
            .await
            .unwrap_or_else(|e| panic!("call {i} within the 30/minute budget should succeed: {e:?}"));
    }
    let err = client
        .tools_call("host.tool_run", json!({"name": "printer", "args": {}}))
        .await
        .expect_err("the 31st host.tool_run call in a minute must be rate limited");
    assert_eq!(err.error_code.as_deref(), Some("rate_limited"));
}
