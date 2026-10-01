//! PRD-mcphost-table-context-and-sql-passthrough
//! AC13 (P2) — Given a 5,000-byte SELECT, When it runs, Then the log row
//! stores the first 4,096 bytes with `truncated: true` and the query
//! itself is not refused for length.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn long_sql_is_logged_truncated_but_query_still_runs() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "SqlPass AC13 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call("host.table.create", json!({"name": "t", "columns": {"x": "text"}}))
        .await
        .expect("create");

    let padding = "a".repeat(5_000);
    let long_sql = format!("SELECT * FROM t WHERE x = '{padding}'");
    assert!(long_sql.len() > 5_000, "sanity: sql must exceed 5,000 bytes");

    let result = extract_structured(
        &client.tools_call("host.table.query", json!({"sql": long_sql.clone()})).await.expect(
            "a long query must not be refused for length",
        ),
    );
    assert_eq!(result["rows"].as_array().expect("rows").len(), 0, "result: {result}");

    let log = extract_structured(
        &client.tools_call("host.table.query_log", json!({"limit": 1})).await.expect("query_log"),
    );
    let entry = &log["entries"][0];
    assert_eq!(entry["truncated"], true, "entry: {entry}");
    let stored_sql = entry["sql"].as_str().expect("sql");
    assert_eq!(stored_sql.len(), 4_096, "stored sql must be exactly 4,096 bytes: got {}", stored_sql.len());
    assert_eq!(stored_sql, &long_sql[..4_096], "stored sql must be the first 4,096 bytes of the original");
}
