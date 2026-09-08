//! PRD-mcphost-result-envelope-contract AC3 — Given a python-kind tool
//! declaring `row_count` that returns the bare integer 42, When called,
//! Then `result.payload.row_count` is 42 and
//! `result.payload._envelope_warning` names the scalar promotion.

mod common;
use common::{TestServer, extract_structured, poll_until_ready, python_kind_registry, signup};
use mcphost::sandbox;
use serde_json::json;
use std::time::Duration;

#[tokio::test]
async fn bare_scalar_return_promotes_to_first_declared_field_with_warning() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs_dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (ns, key) = signup(&server.base_url, "Envelope AC3 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    let spec = json!({
        "source": "def main(args):\n    return 42\n",
        "args_schema": {"type": "object"},
        "outputs": ["row_count"],
    });
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "counter", "kind": "python", "spec": spec}),
        )
        .await
        .expect("publish ok");

    let result = poll_until_ready(
        &client,
        &format!("{ns}.counter"),
        json!({}),
        Duration::from_secs(10),
    )
    .await
    .unwrap_or_else(|e| panic!("call must succeed: {} {}", e.code, e.message));
    let structured = extract_structured(&result);

    assert_eq!(
        structured["payload"]["row_count"], 42,
        "result.payload.row_count must be 42: {structured}"
    );
    assert!(
        structured["payload"]["_envelope_warning"]
            .as_str()
            .is_some_and(|w| w.contains("row_count")),
        "result.payload._envelope_warning must name the scalar promotion: {structured}"
    );
}
