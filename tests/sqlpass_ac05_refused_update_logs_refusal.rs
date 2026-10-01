//! PRD-mcphost-table-context-and-sql-passthrough
//! AC5 — Given an `UPDATE` statement, When `host.table.query` refuses it,
//! Then no table row changes, the caller gets the existing read-only
//! refusal, and the log row carries that refusal's `error_code` with null
//! `row_count`.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn refused_update_writes_nothing_and_logs_the_refusal() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "SqlPass AC5 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call(
            "host.table.create",
            json!({"name": "expenses", "columns": {"amount": "real"}}),
        )
        .await
        .expect("create");
    client
        .tools_call("host.table.append", json!({"table": "expenses", "rows": [{"amount": 1.0}]}))
        .await
        .expect("append");

    let update_sql = "UPDATE expenses SET amount = 999";
    let err = client
        .tools_call("host.table.query", json!({"sql": update_sql}))
        .await
        .expect_err("UPDATE must be refused");
    assert_eq!(err.error_code.as_deref(), Some("table_query_rejected"), "err: {err:?}");

    // Read the log for the refused call before issuing the verification
    // SELECT below (which would otherwise become the newest log entry).
    let log = extract_structured(
        &client.tools_call("host.table.query_log", json!({"limit": 1})).await.expect("query_log"),
    );
    let entry = &log["entries"][0];
    assert_eq!(entry["sql"], update_sql, "log entry: {entry}");
    assert!(entry["row_count"].is_null(), "log entry: {entry}");
    assert_eq!(entry["error_code"], "table_query_rejected", "log entry: {entry}");
    assert!(entry["error_message"].as_str().is_some(), "log entry: {entry}");

    // No row changed.
    let rows = extract_structured(
        &client
            .tools_call("host.table.query", json!({"sql": "SELECT amount FROM expenses"}))
            .await
            .expect("select after refused update"),
    );
    assert_eq!(rows["rows"][0]["amount"], 1.0, "rows: {rows}");
}
