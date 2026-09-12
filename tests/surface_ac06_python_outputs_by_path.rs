//! PRD-mcphost-surface-fluidity AC6 — Given a python tool returning
//! `{"data": {"status": "ok"}}` with `outputs: {"ingestion_status":
//! "$.data.status"}`, When called, Then `result.payload.ingestion_status`
//! is `"ok"` and `host.tool_test` reports it found at that path.
//!
//! Closes mcphost-spec-output-paths requirement 7 (non-goal 3 deferred this
//! to a later PRD): the python kind's map-form `outputs` now extracts by
//! path, the same grammar and the same envelope report the http kind
//! already gets from it (see `tests/specpath_ac01_map_form_normalizes_and_publishes.rs`
//! for that kind's own version of this test).

use crate::common;
use common::{
    McpClient, TempDataDir, TestServer, extract_structured, poll_until_ready, python_kind_registry,
    signup,
};
use mcphost::sandbox;
use serde_json::json;
use std::time::Duration;

#[tokio::test]
async fn map_form_outputs_resolves_by_path_and_tool_test_reports_it_found() {
    // See python_ac01's own comment: this test runs a real sandboxed python
    // tool, which needs unprivileged user namespaces -- not guaranteed on
    // GitHub's hosted runners.
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs_dir = TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (ns, key) = signup(&server.base_url, "Surface AC6 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let spec = json!({
        "source": "def main(args):\n    return {\"data\": {\"status\": \"ok\"}}\n",
        "args_schema": {"type": "object"},
        "outputs": {"ingestion_status": "$.data.status"},
    });
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "ingestor", "kind": "python", "spec": spec}),
        )
        .await
        .expect("publish must succeed for the map form of outputs");

    // The real call: the declared path resolves into result.payload.
    let result = poll_until_ready(
        &client,
        &format!("{ns}.ingestor"),
        json!({}),
        Duration::from_secs(10),
    )
    .await
    .unwrap_or_else(|e| panic!("call must succeed: {} {}", e.code, e.message));
    let structured = extract_structured(&result);
    assert_eq!(
        structured["payload"]["ingestion_status"], "ok",
        "the map form's path $.data.status must have been parsed and read: {structured}"
    );

    // host.tool_test's own envelope report names the field found, not
    // missing -- proof the report reads the same path-resolved payload.
    let tool_test_result = poll_until_ready(
        &client,
        "host.tool_test",
        json!({"name": "ingestor", "args": {}}),
        Duration::from_secs(10),
    )
    .await
    .unwrap_or_else(|e| panic!("host.tool_test must succeed: {} {}", e.code, e.message));
    let tool_test_structured = extract_structured(&tool_test_result);

    assert_eq!(
        tool_test_structured["envelope"]["found_at_contract_path"],
        json!(["ingestion_status"]),
        "host.tool_test must report ingestion_status found: {tool_test_structured}"
    );
    assert_eq!(tool_test_structured["envelope"]["missing"], json!([]));
    assert_eq!(tool_test_structured["envelope"]["green"], true);
}
