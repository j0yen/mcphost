//! PRD-mcphost-dry-run-on-write-verbs
//! AC4 -- Given `host.state.delete_rows {table: "jobs", where: {status:
//! "done"}, dry_run: true}` matching 37 of 50 rows, When called, Then
//! `would.rows == [{table: "jobs", delete: 37, sample: <3 rows>}]` and the
//! table still has 50 rows; Given the filter matches 0 rows, Then
//! `would.rows[0].delete == 0` and no error.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::{Value, json};

async fn count_rows(client: &McpClient) -> usize {
    let result = client
        .tools_call("host.state.query", json!({"table": "jobs"}))
        .await
        .expect("state query");
    extract_structured(&result)["rows"].as_array().expect("rows").len()
}

#[tokio::test]
async fn dry_run_delete_rows_counts_the_match_and_deletes_nothing() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Dryv AC4 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call(
            "host.state.table_create",
            json!({"name": "jobs", "schema": {"n": "integer", "status": "text"}}),
        )
        .await
        .expect("table_create");
    let rows: Vec<Value> = (0..50)
        .map(|n| json!({"n": n, "status": if n < 37 { "done" } else { "open" }}))
        .collect();
    client
        .tools_call("host.state.insert", json!({"table": "jobs", "rows": rows}))
        .await
        .expect("insert 50");
    assert_eq!(count_rows(&client).await, 50);

    let result = client
        .tools_call(
            "host.state.delete_rows",
            json!({"table": "jobs", "where": {"status": "done"}, "dry_run": true}),
        )
        .await
        .expect("dry-run delete_rows");
    let structured = extract_structured(&result);
    let would_rows = structured["would"]["rows"].as_array().expect("would.rows").clone();
    assert_eq!(would_rows.len(), 1, "{structured}");
    assert_eq!(would_rows[0]["table"], json!("jobs"));
    assert_eq!(would_rows[0]["delete"], json!(37));
    let sample = would_rows[0]["sample"].as_array().expect("sample");
    assert_eq!(sample.len(), 3, "{structured}");
    assert!(sample.iter().all(|r| r["status"] == json!("done")), "{sample:?}");
    assert_eq!(structured["_meta"]["dry_run"], json!(true));
    assert_eq!(count_rows(&client).await, 50, "a dry run deletes nothing");

    // The string filter grammar previews the same way.
    let by_string = extract_structured(
        &client
            .tools_call(
                "host.state.delete_rows",
                json!({"table": "jobs", "where": "status = 'done'", "dry_run": true}),
            )
            .await
            .expect("string filter"),
    );
    assert_eq!(by_string["would"]["rows"][0]["delete"], json!(37), "{by_string}");

    // A filter that matches nothing is a count of zero, not an error.
    let none = extract_structured(
        &client
            .tools_call(
                "host.state.delete_rows",
                json!({"table": "jobs", "where": {"status": "archived"}, "dry_run": true}),
            )
            .await
            .expect("zero-match dry run is not an error"),
    );
    assert_eq!(none["would"]["rows"][0]["delete"], json!(0), "{none}");
    assert_eq!(none["would"]["rows"][0]["sample"], json!([]), "{none}");
    assert_eq!(count_rows(&client).await, 50);
}
