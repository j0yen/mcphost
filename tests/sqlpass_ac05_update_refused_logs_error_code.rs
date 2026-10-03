//! PRD-mcphost-table-context-and-sql-passthrough
//! AC5 — Given an `UPDATE` statement, When `host.table.query` refuses it,
//! Then no table row changes, the caller gets the existing read-only
//! refusal, and the log row carries that refusal's `error_code` with null
//! `row_count`.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn update_statement_refused_and_logged_with_error_code() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "SqlPass AC5 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call(
            "host.table.create",
            json!({"name": "widgets", "columns": {"name": "text", "qty": "integer"}}),
        )
        .await
        .expect("create widgets");
    client
        .tools_call(
            "host.table.append",
            json!({"table": "widgets", "rows": [{"name": "sprocket", "qty": 10}]}),
        )
        .await
        .expect("append one row");

    let update_sql = "UPDATE widgets SET qty = 999";
    let err = client
        .tools_call("host.table.query", json!({"sql": update_sql}))
        .await
        .expect_err("UPDATE must be refused");
    let error_code = err.error_code.clone().expect("error carries error_code");
    assert!(!error_code.is_empty(), "error: {err:?}");

    // No row changed.
    let rows = extract_structured(
        &client
            .tools_call("host.table.query", json!({"sql": "SELECT qty FROM widgets"}))
            .await
            .expect("select after refused update"),
    );
    assert_eq!(rows["rows"][0]["qty"], 10, "the UPDATE must never have run: {rows}");

    let log = extract_structured(
        &client.tools_call("host.table.query_log", json!({"limit": 5})).await.expect("query_log"),
    );
    let entry = log["rows"]
        .as_array()
        .expect("rows array")
        .iter()
        .find(|r| r["sql"] == update_sql)
        .unwrap_or_else(|| panic!("no log row for the refused UPDATE: {log}"));
    assert_eq!(entry["error_code"], error_code, "log row: {entry}");
    assert!(entry["row_count"].is_null(), "log row: {entry}");
}
