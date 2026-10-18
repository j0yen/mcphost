//! PRD-mcphost-usage-in-every-response AC7 -- Given `host.tool_test` and
//! `host.tool_run`, When they succeed, Then neither result carries
//! `_meta.usage` and `host.usage.calls` is unchanged.
//!
//! `host.tool_run` is unsupported by the echo kind, so this publishes a real
//! sandboxed python tool (same user-namespace guard as every python-kind test).

use crate::common;
use common::{McpClient, TestServer, extract_structured, poll_until_ready, python_kind_registry, signup};
use mcphost::sandbox;
use serde_json::json;
use std::time::Duration;

#[tokio::test]
async fn tool_test_and_tool_run_carry_no_usage_and_leave_calls_unchanged() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs_dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (ns, key) = signup(&server.base_url, "Meta No Usage").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    let spec = json!({
        "source": "def main(args):\n    return {\"diagnosis\": \"looks fine\"}\n",
        "args_schema": {"type": "object"},
        "outputs": ["diagnosis"],
    });
    client
        .tools_call("host.tool_publish", json!({"name": "diagnoser", "kind": "python", "spec": spec}))
        .await
        .expect("publish");

    // Warm the build with an ordinary call, which DOES carry `_meta.usage`.
    let real = poll_until_ready(&client, &format!("{ns}.diagnoser"), json!({}), Duration::from_secs(10)).await.expect("warm call");
    assert!(real["_meta"]["usage"]["calls_per_day"].is_object(), "a real call carries _meta.usage: {real}");

    let calls = || async {
        let usage = client.tools_call("host.usage", json!({})).await.expect("host.usage");
        extract_structured(&usage)["calls"].as_i64().expect("calls")
    };
    let before = calls().await;

    let tested = client
        .tools_call("host.tool_test", json!({"name": "diagnoser", "args": {}}))
        .await
        .expect("host.tool_test succeeds");
    let ran = client
        .tools_call("host.tool_run", json!({"name": "diagnoser", "args": {}}))
        .await
        .expect("host.tool_run succeeds");
    for (name, result) in [("host.tool_test", &tested), ("host.tool_run", &ran)] {
        assert!(result.get("_meta").and_then(|m| m.get("usage")).is_none(), "{name} carries _meta.usage: {result}");
    }

    assert_eq!(calls().await, before, "host.usage.calls moved after tool_test/tool_run");
}
