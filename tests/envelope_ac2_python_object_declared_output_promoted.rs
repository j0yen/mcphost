//! PRD-mcphost-result-envelope-contract AC2 — Given a python-kind tool
//! declaring `diagnosis` that returns `{"analysis":{"diagnosis":"…"}}`,
//! When called, Then `result.payload.diagnosis` is present with that value.

use crate::common;
use common::{TestServer, extract_structured, poll_until_ready, python_kind_registry, signup};
use mcphost::sandbox;
use serde_json::json;
use std::time::Duration;

#[tokio::test]
async fn declared_field_nested_under_analysis_lands_at_result_payload() {
    // See python_ac01's own comment: this test runs a real sandboxed python
    // tool, which needs unprivileged user namespaces -- not guaranteed on
    // GitHub's hosted runners.
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs_dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (ns, key) = signup(&server.base_url, "Envelope AC2 Tenant").await;
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

    let result = poll_until_ready(
        &client,
        &format!("{ns}.diagnoser"),
        json!({}),
        Duration::from_secs(10),
    )
    .await
    .unwrap_or_else(|e| panic!("call must succeed: {} {}", e.code, e.message));
    let structured = extract_structured(&result);

    assert_eq!(
        structured["payload"]["diagnosis"], "looks fine",
        "result.payload.diagnosis must be present: {structured}"
    );
    // Existing nested shape stays (Migration/compatibility: additive).
    assert_eq!(structured["analysis"]["diagnosis"], "looks fine");
}
