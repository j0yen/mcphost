//! PRD-mcphost-chain-host-steps AC1 (P0) — Given a chain whose step 2 is
//! `{"tool":"host.table.append","args":{"table":"runs","rows":"$.prev.result.rows"}}`,
//! When published and called with input that step 1 turns into one row,
//! Then the run status is `done`, `steps[1].result.appended == 1`, and
//! `host.table.query` returns the row.
//!
//! Step 1 is an `echo` tool (permissive schema): it validates and returns
//! its own arguments verbatim, so the chain's own call args become step 1's
//! result directly, with no real tool-logic of its own to prove -- the
//! point under test is step 2, the first `host.*` verb a chain step can
//! name (`kinds::compose_call`'s new resolution order, requirement 1).

use crate::common;
use common::{TestServer, chain_kind_registry, publish, signup};
use serde_json::json;

#[tokio::test]
async fn host_table_append_step_runs_and_the_row_is_queryable() {
    let server = TestServer::start_with_kinds(chain_kind_registry()).await;
    let (ns, key) = signup(&server.base_url, "Chain Host Step Tenant").await;
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
            json!({
                "name": "runs",
                "columns": {"id": "integer", "note": "text"},
            }),
        )
        .await
        .expect("host.table.create must succeed");

    let chain_spec = json!({
        "steps": [
            {"tool": "fetch_rows", "args": {"rows": "$.input.rows"}},
            {"tool": "host.table.append", "args": {"table": "runs", "rows": "$.prev.result.rows"}}
        ]
    });
    let chain = publish(&client, "pipeline", "chain", chain_spec).await;
    assert_eq!(chain, format!("{ns}.pipeline"));

    let call = client
        .tools_call(
            &chain,
            json!({"rows": [{"id": 1, "note": "hello"}]}),
        )
        .await
        .expect("a chain call with one host.table.append step must succeed");
    let structured = common::extract_structured(&call);

    assert!(
        structured["failed_steps"].as_array().unwrap().is_empty(),
        "no step should have failed: {structured}"
    );
    let steps = structured["steps"].as_array().expect("steps trace");
    assert_eq!(steps.len(), 2);
    assert_eq!(steps[1]["tool"], json!("host.table.append"));
    assert_eq!(steps[1]["status"], json!("done"));
    assert_eq!(
        steps[1]["result"]["appended"], json!(1),
        "steps[1].result.appended must be 1: {structured}"
    );

    // "the run status is done": every synchronous call also writes a `runs`
    // row (PRD-mcphost-runs-and-jobs requirement 2, already landed) --
    // checked directly against that row rather than inventing new RPC
    // surface this PRD doesn't otherwise need.
    let db_path = server.data_dir.0.join("mcphost.db");
    let conn = rusqlite::Connection::open(&db_path).expect("open raw db");
    let status: String = conn
        .query_row(
            "SELECT status FROM runs WHERE tool_name = 'pipeline' ORDER BY id DESC LIMIT 1",
            [],
            |r| r.get(0),
        )
        .expect("a runs row for the chain call");
    assert_eq!(status, "done");

    let query = client
        .tools_call(
            "host.table.query",
            json!({"sql": "SELECT id, note FROM runs"}),
        )
        .await
        .expect("host.table.query must succeed");
    let query_structured = common::extract_structured(&query);
    assert_eq!(
        query_structured["rows"],
        json!([{"id": 1, "note": "hello"}]),
        "the appended row must be queryable back: {query_structured}"
    );
}
