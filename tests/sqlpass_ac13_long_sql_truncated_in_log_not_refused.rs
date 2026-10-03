//! PRD-mcphost-table-context-and-sql-passthrough
//! AC13 — Given a 5,000-byte SELECT, When it runs, Then the log row
//! stores the first 4,096 bytes with `truncated: true` and the query
//! itself is not refused for length.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn long_select_is_not_refused_and_log_stores_truncated_sql() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "SqlPass AC13 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call("host.table.create", json!({"name": "t", "columns": {"n": "integer"}}))
        .await
        .expect("create t");
    client
        .tools_call("host.table.append", json!({"table": "t", "rows": [{"n": 1}]}))
        .await
        .expect("append one row");

    let padding = "x".repeat(5_000);
    let long_sql = format!("SELECT '{padding}' AS val FROM t");
    assert!(long_sql.len() >= 5_000, "sanity: sql is at least 5,000 bytes ({})", long_sql.len());

    let result = extract_structured(
        &client
            .tools_call("host.table.query", json!({"sql": long_sql.clone()}))
            .await
            .expect("a long SELECT must not be refused for length"),
    );
    assert_eq!(result["rows"].as_array().expect("rows array").len(), 1, "result: {result}");

    let log = extract_structured(
        &client.tools_call("host.table.query_log", json!({"limit": 1})).await.expect("query_log"),
    );
    let entry = &log["rows"][0];
    assert_eq!(entry["truncated"], true, "log row: {entry}");
    let stored_sql = entry["sql"].as_str().expect("sql string");
    assert_eq!(stored_sql.len(), 4_096, "stored sql must be exactly the first 4096 bytes, got {}", stored_sql.len());
    assert_eq!(stored_sql, &long_sql[..4_096], "stored sql must be a prefix of the submitted query");
    assert!(entry["error_code"].is_null(), "log row: {entry}");
}
