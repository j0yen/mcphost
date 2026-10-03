//! PRD-mcphost-dry-run-side-effects
//! AC5 — Given a chain whose step is `host.table.append` (after
//! PRD-mcphost-chain-host-steps), When `host.tool_test` runs the chain,
//! Then the step's write appears in `dry_run.writes` and the table is
//! unchanged.

use crate::common;
use common::{chain_kind_registry, extract_structured, publish, signup};
use serde_json::json;

#[tokio::test]
async fn tool_test_on_a_chain_with_a_host_table_append_step_rolls_back_and_reports_the_write() {
    let server = common::TestServer::start_with_kinds(chain_kind_registry()).await;
    let (_ns, key) = signup(&server.base_url, "Dry Run AC5 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call(
            "host.table.create",
            json!({"name": "runs", "columns": {"slug": "text"}}),
        )
        .await
        .expect("table create ok");

    let chain_spec = json!({
        "steps": [
            {"tool": "host.table.append", "args": {"table": "runs", "rows": [{"slug": "chain-dryrun"}]}},
        ]
    });
    publish(&client, "pipeline", "chain", chain_spec).await;

    let result = client
        .tools_call("host.tool_test", json!({"name": "pipeline", "args": {}}))
        .await
        .expect("tool_test ok");
    let structured = extract_structured(&result);

    assert_eq!(
        structured["dry_run"]["writes"],
        json!([{"store": "table", "op": "append", "table": "runs", "rows": 1}]),
        "{structured:?}"
    );
    assert_eq!(structured["dry_run"]["rolled_back"], json!(true), "{structured:?}");
    assert_eq!(structured["dry_run"]["delivered"], json!(false), "{structured:?}");

    let queried = client
        .tools_call("host.table.query", json!({"sql": "SELECT COUNT(*) AS n FROM runs"}))
        .await
        .expect("table query ok");
    let rows = extract_structured(&queried)["rows"].as_array().expect("rows array").clone();
    assert_eq!(
        rows[0]["n"],
        json!(0),
        "a dry run must not persist the chain step's appended row: {rows:?}"
    );
}
