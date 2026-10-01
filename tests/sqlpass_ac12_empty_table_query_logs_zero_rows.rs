//! PRD-mcphost-table-context-and-sql-passthrough
//! AC12 (P1) — Given an empty declared table, When `SELECT * FROM t` runs,
//! Then zero rows return and the log row carries `row_count` 0 and null
//! `error_code`.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn empty_table_query_logs_zero_row_count_and_no_error() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "SqlPass AC12 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call("host.table.create", json!({"name": "t", "columns": {"x": "integer"}}))
        .await
        .expect("create");

    let result = extract_structured(
        &client.tools_call("host.table.query", json!({"sql": "SELECT * FROM t"})).await.expect("query"),
    );
    assert_eq!(result["rows"].as_array().expect("rows").len(), 0, "result: {result}");

    let log = extract_structured(
        &client.tools_call("host.table.query_log", json!({"limit": 1})).await.expect("query_log"),
    );
    let entry = &log["entries"][0];
    assert_eq!(entry["row_count"], 0, "entry: {entry}");
    assert!(entry["error_code"].is_null(), "entry: {entry}");
}
