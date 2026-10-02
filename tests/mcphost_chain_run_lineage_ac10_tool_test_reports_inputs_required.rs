//! PRD-mcphost-chain-run-lineage AC10 (P0) — Given `host.tool_test` on the
//! chain from AC-1, When called, Then the report includes
//! `inputs_required == ["url","region"]`.

use crate::common;
use common::{chain_kind_registry, extract_structured, publish, signup, TestServer};
use serde_json::json;

#[tokio::test]
async fn tool_test_dry_run_reports_inputs_required() {
    let server = TestServer::start_with_kinds(chain_kind_registry()).await;
    let (ns, key) = signup(&server.base_url, "AC10 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    let schema = json!({"type": "object"});
    publish(&client, "fetch_data", "echo", json!({"schema": schema})).await;
    publish(&client, "transform", "echo", json!({"schema": schema})).await;
    publish(&client, "write", "echo", json!({"schema": schema})).await;

    let chain_spec = json!({
        "steps": [
            {"tool": "fetch_data", "args": {"url": "$.input.url"}},
            {"tool": "transform", "args": {"rows": "$.prev.result.rows"}},
            {"tool": "write", "args": {"rows": "$.prev.result.rows", "region": "$.input.region"}},
        ]
    });
    let chain = publish(&client, "daily_pipeline", "chain", chain_spec).await;
    assert_eq!(chain, format!("{ns}.daily_pipeline"));

    // Deliberately incomplete args -- a real call would refuse
    // `compose_input_missing`, but `host.tool_test`'s dry run must still
    // report `inputs_required` (and resolve what it can) rather than
    // refusing outright.
    let report = extract_structured(
        &client
            .tools_call("host.tool_test", json!({"name": "daily_pipeline", "args": {}}))
            .await
            .expect("tool_test ok"),
    );
    assert_eq!(
        report["inputs_required"],
        json!(["url", "region"]),
        "tool_test's dry-run report must include inputs_required: {report}"
    );
}
