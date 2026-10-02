//! PRD-mcphost-chain-run-lineage AC11 (P1) — Given a failed child row, When
//! `host.runs.list(status="failed", trigger="composition")` is read, Then
//! the row carries `step_no` and `parent_tool`.

use crate::common;
use common::{chain_kind_registry, extract_structured, publish, signup, TestServer};
use serde_json::json;

#[tokio::test]
async fn a_failed_child_row_names_its_step_number_and_parent_tool() {
    let server = TestServer::start_with_kinds(chain_kind_registry()).await;
    let (ns, key) = signup(&server.base_url, "AC11 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    let schema = json!({"type": "object"});
    publish(&client, "step_one", "echo", json!({"schema": schema})).await;
    publish(
        &client,
        "inner_broken",
        "chain",
        json!({"steps": [{"tool": "step_one", "args": {"x": "$.input.x"}}]}),
    )
    .await;

    let chain_spec = json!({
        "steps": [
            {"tool": "step_one", "args": {"n": "$.input.n"}},
            {"tool": "inner_broken", "args": {}, "on_error": "stop"},
        ]
    });
    let chain = publish(&client, "pipeline", "chain", chain_spec).await;
    assert_eq!(chain, format!("{ns}.pipeline"));

    client
        .tools_call(&chain, json!({"n": 1}))
        .await
        .expect_err("step two's failure must propagate");

    let failed = extract_structured(
        &client
            .tools_call(
                "host.runs.list",
                json!({"status": "failed", "trigger": "composition"}),
            )
            .await
            .expect("runs.list ok"),
    );
    let rows = failed["runs"].as_array().expect("runs array");
    assert_eq!(rows.len(), 1, "exactly one failed composition child: {failed}");
    let row = &rows[0];
    assert_eq!(row["tool"], json!("inner_broken"));
    assert_eq!(row["step_no"], json!(2));
    assert_eq!(row["parent_tool"], json!("pipeline"));
    assert_eq!(row["error_class"], json!("compose_input_missing"));
}
