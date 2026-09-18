//! PRD-mcphost-tool-run-envelope AC1 — Given a tool whose output matches
//! its declared outputs, When exercised via `tool_call` and via `tool_run`
//! on the same fixture, Then both results carry the fields at
//! `result.payload.<field>` identically, and `tool_run` additionally
//! carries `duration_ms` and `exit_code`.

use crate::common;
use common::{TestServer, extract_structured, poll_until_ready, python_kind_registry, signup};
use mcphost::sandbox;
use serde_json::json;
use std::time::Duration;

#[tokio::test]
async fn call_and_tool_run_place_the_declared_field_identically() {
    // Real sandboxed python tool -- needs unprivileged user namespaces, not
    // guaranteed on GitHub's hosted runners (same guard as every other
    // python_ac*.rs / envelope_ac*.rs test).
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs_dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (ns, key) = signup(&server.base_url, "Runenvelope AC1 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    let spec = json!({
        "source": "def main(args):\n    return {\"analysis\": {\"diagnosis\": \"looks fine\"}}\n",
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

    let call_result = poll_until_ready(
        &client,
        &format!("{ns}.diagnoser"),
        json!({}),
        Duration::from_secs(10),
    )
    .await
    .unwrap_or_else(|e| panic!("call must succeed: {} {}", e.code, e.message));
    let call_structured = extract_structured(&call_result);

    let run_result = client
        .tools_call("host.tool_run", json!({"name": "diagnoser", "args": {}}))
        .await
        .expect("host.tool_run must succeed");
    let run_structured = extract_structured(&run_result);

    assert_eq!(
        call_structured["payload"]["diagnosis"], "looks fine",
        "tool_call's result.payload.diagnosis must be present: {call_structured}"
    );
    assert_eq!(
        run_structured["payload"]["diagnosis"], "looks fine",
        "tool_run's result.payload.diagnosis must be present: {run_structured}"
    );
    assert_eq!(
        call_structured["payload"]["diagnosis"], run_structured["payload"]["diagnosis"],
        "both surfaces must place the declared field identically"
    );

    // tool_run additionally carries run metadata a plain call never has.
    assert!(
        run_structured["duration_ms"].is_number(),
        "tool_run must carry duration_ms: {run_structured}"
    );
    assert_eq!(
        run_structured["exit_code"],
        json!(0),
        "a successful tool_run is exit_code 0: {run_structured}"
    );
}
