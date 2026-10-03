//! PRD-mcphost-chain-host-steps AC4 (P0) — Given a published chain with a
//! `host.table.append` step, When `host.tool_test` runs it, Then the
//! dry-run report lists the step as `resolved: host` and the report
//! carries `side_effects: true` for that step. Here the step's args
//! reach into `$.prev`, which a dry run can't resolve (the prior step is
//! an ordinary tenant tool and is never dispatched), so per
//! PRD-mcphost-dry-run-side-effects AC5 the host step itself also stays
//! undispatched -- the chain's own static report lives under
//! `chain_dry_run` (not `dry_run`, which the result envelope now owns
//! for `{writes, delivered, rolled_back}`).

use crate::common;
use common::{TestServer, chain_kind_registry, publish, signup};
use serde_json::json;

#[tokio::test]
async fn host_table_append_step_dry_run_reports_resolved_host_and_side_effects() {
    let server = TestServer::start_with_kinds(chain_kind_registry()).await;
    let (_ns, key) = signup(&server.base_url, "Chain Dry Run Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    publish(
        &client,
        "fetch_rows",
        "echo",
        json!({"schema": {"type": "object"}}),
    )
    .await;

    client
        .tools_call(
            "host.table.create",
            json!({"name": "runs", "columns": {"id": "integer"}}),
        )
        .await
        .expect("host.table.create must succeed");

    let chain_spec = json!({
        "steps": [
            {"tool": "fetch_rows", "args": {"rows": "$.input.rows"}},
            {"tool": "host.table.append", "args": {"table": "runs", "rows": "$.prev.result.rows"}}
        ]
    });
    publish(&client, "pipeline", "chain", chain_spec).await;

    let result = client
        .tools_call(
            "host.tool_test",
            json!({"name": "pipeline", "args": {"rows": [{"id": 1}]}}),
        )
        .await
        .expect("host.tool_test must succeed -- it never dispatches a step");
    let structured = common::extract_structured(&result);

    assert_eq!(structured["chain_dry_run"], json!(true));
    let steps = structured["steps"].as_array().expect("steps report");
    assert_eq!(steps.len(), 2);

    assert_eq!(steps[0]["tool"], json!("fetch_rows"));
    assert_eq!(steps[0]["resolved"], json!("tenant"));
    assert!(
        steps[0].get("side_effects").is_none(),
        "a tenant-tool step must not carry side_effects: {steps:?}"
    );

    assert_eq!(steps[1]["tool"], json!("host.table.append"));
    assert_eq!(steps[1]["resolved"], json!("host"));
    assert_eq!(steps[1]["side_effects"], json!(true));

    // The dry run never actually dispatched the host step: no row written.
    let query = client
        .tools_call("host.table.query", json!({"sql": "SELECT id FROM runs"}))
        .await
        .expect("host.table.query must succeed");
    let query_structured = common::extract_structured(&query);
    assert_eq!(
        query_structured["rows"],
        json!([]),
        "a dry run must write nothing: {query_structured}"
    );
}
