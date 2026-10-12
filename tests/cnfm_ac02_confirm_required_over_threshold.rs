//! PRD-mcphost-destructive-verb-confirmation-token
//! AC2 -- Given a 5 000-row table and `confirm_rows_min` 100, When
//! `host.state.delete_rows {table, where: {}}` is called without `confirm`,
//! Then `confirm_required` is returned with `dry_run_first.args.dry_run ==
//! true`, `would.rows[0].delete == 5000`, and the table still has 5 000 rows;
//! Given a `where` matching 12 rows, Then the delete runs with no token.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::{Value, json};

async fn seed_jobs(client: &McpClient, total: usize, done: usize) {
    client
        .tools_call(
            "host.state.table_create",
            json!({"name": "jobs", "schema": {"n": "integer", "status": "text"}}),
        )
        .await
        .expect("table_create");
    append_jobs(client, 0, total, done).await;
}

/// Inserts rows `from..to` (ids below `done` are `status: "done"`), in
/// batches under the per-call state-ops ceiling.
async fn append_jobs(client: &McpClient, from: usize, to: usize, done: usize) {
    let mut n = from;
    while n < to {
        let end = (n + 200).min(to);
        let rows: Vec<Value> = (n..end)
            .map(|i| json!({"n": i, "status": if i < done { "done" } else { "open" }}))
            .collect();
        client
            .tools_call("host.state.insert", json!({"table": "jobs", "rows": rows}))
            .await
            .expect("insert batch");
        n = end;
    }
}

async fn count_jobs(client: &McpClient) -> usize {
    let result = client
        .tools_call("host.state.query", json!({"table": "jobs", "limit": 10_000}))
        .await
        .expect("state query");
    extract_structured(&result)["rows"].as_array().expect("rows").len()
}

#[tokio::test]
async fn large_delete_needs_a_token_and_a_small_one_does_not() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Cnfm AC2 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    seed_jobs(&client, 5_000, 12).await;
    assert_eq!(count_jobs(&client).await, 5_000);

    let refused = client
        .tools_call("host.state.delete_rows", json!({"table": "jobs", "where": {}}))
        .await
        .expect_err("an empty filter over 5 000 rows needs a token");
    assert_eq!(refused.error_code.as_deref(), Some("confirm_required"), "{refused:?}");
    let first = &refused.data["dry_run_first"];
    assert_eq!(first["tool"], json!("host.state.delete_rows"), "{refused:?}");
    assert_eq!(first["args"]["dry_run"], json!(true), "{refused:?}");
    assert_eq!(first["args"]["table"], json!("jobs"), "{refused:?}");
    assert_eq!(refused.data["would"]["rows"][0]["delete"], json!(5_000), "{refused:?}");
    assert_eq!(refused.data["confirm_rows_min"], json!(100), "{refused:?}");
    assert_eq!(count_jobs(&client).await, 5_000, "a refused delete deletes nothing");

    // The error names a call that works verbatim.
    let dry = client
        .tools_call(
            first["tool"].as_str().expect("tool"),
            first["args"].clone(),
        )
        .await
        .expect("the named dry run is callable as-is");
    let dry = extract_structured(&dry);
    assert_eq!(dry["would"]["rows"][0]["delete"], json!(5_000), "{dry}");
    assert!(dry["confirm"].as_str().is_some_and(|c| c.starts_with("c_")), "{dry}");

    // 12 rows is under the threshold: no token needed.
    let small = client
        .tools_call("host.state.delete_rows", json!({"table": "jobs", "where": {"status": "done"}}))
        .await
        .expect("12 matching rows run without a token");
    assert_eq!(extract_structured(&small)["deleted"], json!(12));
    assert_eq!(count_jobs(&client).await, 4_988);
}
