//! PRD-mcphost-chain-host-steps AC7 (P1) — Given a free-plan tenant at
//! `table_rows_max`, When a chain's `host.table.append` step would exceed
//! it, Then the run fails at that step with the same error class a direct
//! `host.table.append` call returns.

use crate::common;
use common::{TestServer, chain_kind_registry, publish, signup};
use serde_json::json;

#[tokio::test]
async fn host_table_append_step_fails_with_the_same_class_as_a_direct_call() {
    let server = TestServer::start_with_kinds(chain_kind_registry()).await;
    let (_ns, key) = signup(&server.base_url, "Chain Host Step Quota Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    let rows_max = server.state.plans.get("free").expect("free plan").table_rows_max;

    client
        .tools_call(
            "host.table.create",
            json!({"name": "runs", "columns": {"id": "integer"}}),
        )
        .await
        .expect("host.table.create must succeed");

    // Fill the table to exactly `table_rows_max` in one batch -- the same
    // "measure before writing" quota `table_append` enforces for any
    // caller, direct or step.
    let rows: Vec<_> = (0..rows_max).map(|i| json!({"id": i})).collect();
    client
        .tools_call("host.table.append", json!({"table": "runs", "rows": rows}))
        .await
        .expect("filling to exactly table_rows_max must succeed");

    // Baseline: what a direct call over the quota returns.
    let direct_err = client
        .tools_call(
            "host.table.append",
            json!({"table": "runs", "rows": [{"id": rows_max}]}),
        )
        .await
        .expect_err("a direct append past table_rows_max must fail");

    let chain_spec = json!({
        "steps": [
            {"tool": "host.table.append", "args": {"table": "runs", "rows": [{"id": rows_max + 1}]}}
        ]
    });
    let chain = publish(&client, "pipeline", "chain", chain_spec).await;

    let chain_err = client
        .tools_call(&chain, json!({}))
        .await
        .expect_err("a chain step past table_rows_max must fail the run");

    assert_eq!(
        chain_err.error_code, direct_err.error_code,
        "the chain step's error class must match a direct call's: chain={chain_err:?} direct={direct_err:?}"
    );
    assert_eq!(
        chain_err.data["limit"]["name"], direct_err.data["limit"]["name"],
        "both must name the same exceeded limit: chain={chain_err:?} direct={direct_err:?}"
    );
    assert_eq!(
        chain_err.data["limit"]["name"], json!("table_rows_max"),
        "the exceeded limit must be table_rows_max: {chain_err:?}"
    );
    assert_eq!(chain_err.data["failed_step"], json!(1));
    assert_eq!(chain_err.data["step_tool"], json!("host.table.append"));
}
