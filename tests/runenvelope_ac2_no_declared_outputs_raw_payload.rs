//! PRD-mcphost-tool-run-envelope AC2 — Given a tool with no declared
//! outputs, When `tool_run` executes it, Then raw output lands under
//! `result.payload` exactly as a call would place it.

use crate::common;
use common::{TestServer, extract_structured, poll_until_ready, python_kind_registry, signup};
use mcphost::sandbox;
use serde_json::json;
use std::time::Duration;

#[tokio::test]
async fn undeclared_tool_run_result_lands_at_payload_unchanged() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs_dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (ns, key) = signup(&server.base_url, "Runenvelope AC2 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    // No `outputs` key at all -- the spec declares nothing.
    let spec = json!({
        "source": "def main(args):\n    return {\"raw\": \"value\", \"count\": 3}\n",
        "args_schema": {"type": "object"},
    });
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "plain", "kind": "python", "spec": spec}),
        )
        .await
        .expect("publish ok");

    // Warm the build via an ordinary call, whose own (undeclared, unpromoted)
    // return value is the raw tool output itself -- the baseline "as a call
    // would place it" this AC compares tool_run against.
    let call_result = poll_until_ready(
        &client,
        &format!("{ns}.plain"),
        json!({}),
        Duration::from_secs(10),
    )
    .await
    .unwrap_or_else(|e| panic!("call must succeed: {} {}", e.code, e.message));
    let call_structured = extract_structured(&call_result);
    assert_eq!(call_structured["raw"], "value", "sanity: the call's own raw return value");
    assert_eq!(call_structured["count"], 3);

    let run_result = client
        .tools_call("host.tool_run", json!({"name": "plain", "args": {}}))
        .await
        .expect("host.tool_run must succeed");
    let run_structured = extract_structured(&run_result);

    assert_eq!(
        run_structured["payload"]["raw"], "value",
        "raw output must land under result.payload unpromoted: {run_structured}"
    );
    assert_eq!(run_structured["payload"]["count"], 3);
    assert_eq!(
        run_structured["payload"], call_structured,
        "tool_run's payload must equal exactly what an ordinary call returned, \
         since neither promotes anything for a tool with no declared outputs"
    );
}
