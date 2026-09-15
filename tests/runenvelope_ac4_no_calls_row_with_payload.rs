//! PRD-mcphost-tool-run-envelope AC4 — Given `tool_run` executes, When the
//! calls table is inspected, Then no row was recorded (the no-metering
//! property is unchanged by this PRD's envelope change).

use crate::common;
use common::{TestServer, extract_structured, poll_until_ready, python_kind_registry, signup};
use mcphost::sandbox;
use serde_json::json;
use std::time::Duration;

#[tokio::test]
async fn tool_run_with_declared_outputs_still_writes_no_calls_row() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs_dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (ns, key) = signup(&server.base_url, "Runenvelope AC4 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    let spec = json!({
        "source": "def main(args):\n    return {\"diagnosis\": \"looks fine\"}\n",
        "args_schema": {"type": "object"},
        "outputs": ["diagnosis"],
    });
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "diagnoser", "kind": "python", "spec": spec}),
        )
        .await
        .expect("publish ok");

    // Warm the build via an ordinary call (which DOES write a `calls` row)
    // before measuring tool_run's delta, same pattern as
    // warmpool_ac06_ac07_tool_run.rs.
    let _ = poll_until_ready(
        &client,
        &format!("{ns}.diagnoser"),
        json!({}),
        Duration::from_secs(10),
    )
    .await;

    let usage_calls = |result: &serde_json::Value| -> i64 {
        extract_structured(result)["calls"].as_i64().unwrap_or(-1)
    };
    let before = client.tools_call("host.usage", json!({})).await.expect("usage before");
    let calls_before = usage_calls(&before);

    let run_result = client
        .tools_call("host.tool_run", json!({"name": "diagnoser", "args": {}}))
        .await
        .expect("host.tool_run must succeed");
    let structured = extract_structured(&run_result);
    // Sanity: this PRD's own envelope placement landed too, not just the
    // no-metering property.
    assert_eq!(structured["payload"]["diagnosis"], "looks fine");

    let after = client.tools_call("host.usage", json!({})).await.expect("usage after");
    let calls_after = usage_calls(&after);
    assert_eq!(
        calls_before, calls_after,
        "host.tool_run must write no `calls` row, envelope change or not"
    );
}
