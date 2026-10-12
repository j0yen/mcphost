//! PRD-mcphost-table-schema-violation-names-the-remedy
//! AC1 — Given a table declared `(ts timestamp, amount real)`, When
//! `host.table.append` receives a row with an undeclared column `op`, Then
//! the error is `table_schema_violation`, the message lists the declared
//! columns (with a `nearest` clause only when a declared name scores at
//! least `partial`), `data.kind` is `unknown_column`, `data.declared` lists
//! both columns with types, and nothing is written.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

async fn setup(label: &str) -> (TestServer, McpClient) {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, label).await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    client
        .tools_call(
            "host.table.create",
            json!({"name": "events", "columns": {"ts": "timestamp", "amount": "real"}}),
        )
        .await
        .expect("table create");
    (server, client)
}

#[tokio::test]
async fn unknown_column_names_declared_columns_and_writes_nothing() {
    let (_server, client) = setup("TSVR AC1").await;
    let err = client
        .tools_call(
            "host.table.append",
            json!({"table": "events", "rows": [{"op": "sync", "ts": "2026-10-12T00:00:00Z"}]}),
        )
        .await
        .expect_err("undeclared column must be rejected");
    assert_eq!(err.error_code.as_deref(), Some("table_schema_violation"), "{err:?}");
    // Columns are stored and listed in name order.
    assert!(
        err.message.contains(
            "column 'op' is not declared on table 'events' (declared: amount real, ts timestamp)"
        ),
        "{}",
        err.message
    );
    assert!(!err.message.contains("nearest"), "{}", err.message);
    assert_eq!(err.data["kind"], json!("unknown_column"));
    assert_eq!(err.data["field"], json!("op"));
    assert_eq!(err.data["table"], json!("events"));
    assert_eq!(
        err.data["declared"],
        json!([{"name": "amount", "type": "real"}, {"name": "ts", "type": "timestamp"}])
    );
    assert!(err.data["nearest"].is_null());
    assert!(err.data["expected"].is_null());

    let result = extract_structured(
        &client
            .tools_call("host.table.query", json!({"sql": "SELECT COUNT(*) AS n FROM events"}))
            .await
            .expect("query"),
    );
    assert_eq!(result["rows"][0]["n"], json!(0), "{result}");
}

#[tokio::test]
async fn near_miss_name_gets_a_nearest_clause() {
    let (_server, client) = setup("TSVR AC1 nearest").await;
    let err = client
        .tools_call("host.table.append", json!({"table": "events", "rows": [{"amout": 1.5}]}))
        .await
        .expect_err("undeclared column must be rejected");
    assert!(err.message.ends_with("; nearest: 'amount')"), "{}", err.message);
    assert_eq!(err.data["nearest"], json!("amount"));
}
