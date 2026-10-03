//! PRD-mcphost-drift-review
//! AC12 -- Given a `python` tool, When it calls `mcphost.drift.reviews()`,
//! Then it receives the same list the tool returns.

use crate::common;
use common::{TestServer, extract_structured, poll_until_ready, python_kind_registry, signup};
use mcphost::sandbox;
use serde_json::json;
use std::time::Duration;

#[tokio::test]
async fn python_tool_reads_drift_reviews_via_mcphost_drift_reviews() {
    // Same sandbox-dependent skip convention as every other python-kind
    // integration test (tests/python_ac11_secret_redaction.rs etc.).
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs_dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (ns, key) = signup(&server.base_url, "Drift AC12 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call("host.table.create", json!({"name": "widgets", "columns": {"n": "integer"}}))
        .await
        .expect("create widgets");
    client
        .tools_call("host.table.model_set", json!({"table": "widgets", "key": "description", "value": "note"}))
        .await
        .expect("model_set");
    mcphost::tables_model::tick_once(&server.state).await.expect("tick_once");

    let spec = json!({
        "source": "import mcphost\ndef main(args):\n    return {\"reviews\": mcphost.drift.reviews()}\n",
        "args_schema": {"type": "object", "properties": {}},
    });
    client
        .tools_call("host.tool_publish", json!({"name": "reader", "kind": "python", "spec": spec}))
        .await
        .expect("publish ok");

    let result = poll_until_ready(&client, &format!("{ns}.reader"), json!({}), Duration::from_secs(10))
        .await
        .unwrap_or_else(|e| panic!("call must succeed: {} {}", e.code, e.message));
    let structured = extract_structured(&result);
    let from_sandbox = structured["reviews"].clone();

    let from_tool = extract_structured(
        &client.tools_call("host.drift.reviews", json!({})).await.expect("host.drift.reviews ok"),
    );

    assert_eq!(from_sandbox, from_tool, "mcphost.drift.reviews() must return exactly what the tool returns");
}
