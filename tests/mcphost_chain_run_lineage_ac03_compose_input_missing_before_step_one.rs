//! PRD-mcphost-chain-run-lineage AC3 (P0) — Given the chain from AC-1, When
//! `host.tool_call(name, args={})` is called, Then it fails
//! `compose_input_missing` with `data.missing == ["url","region"]`,
//! `data.step == 1`, and `host.runs.list(include_children=true)` shows zero
//! `trigger="composition"` rows for this tenant.

use crate::common;
use common::{chain_kind_registry, publish, signup, TestServer};
use serde_json::json;

#[tokio::test]
async fn a_call_missing_every_input_refuses_before_step_one_and_writes_no_children() {
    let server = TestServer::start_with_kinds(chain_kind_registry()).await;
    let (ns, key) = signup(&server.base_url, "AC3 Tenant").await;
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

    let err = client
        .tools_call(&chain, json!({}))
        .await
        .expect_err("a call missing every $.input.* value must be refused");
    assert_eq!(err.error_code.as_deref(), Some("compose_input_missing"));
    assert_eq!(err.data["missing"], json!(["url", "region"]));
    assert_eq!(err.data["step"], json!(1));
    assert_eq!(err.data["tool"], json!("fetch_data"));

    let runs = common::extract_structured(
        &client
            .tools_call("host.runs.list", json!({"include_children": true}))
            .await
            .expect("runs.list ok"),
    );
    let composition_rows: Vec<&serde_json::Value> = runs["runs"]
        .as_array()
        .expect("runs array")
        .iter()
        .filter(|r| r["trigger"] == json!("composition"))
        .collect();
    assert!(
        composition_rows.is_empty(),
        "zero steps ran, so zero composition child rows must exist: {runs}"
    );
}
