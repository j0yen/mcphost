//! PRD-mcphost-tenant-state
//! AC5 — Given a row insert with `last_value: "high"` (wrong type), When it
//! runs, Then the error is structured `state_schema_violation` naming the
//! column and expected type, and nothing is written.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn wrong_type_is_rejected_and_writes_nothing() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Monitor").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call(
            "host.state.table_create",
            json!({"name": "alerts", "schema": {"metric": "text", "last_value": "real"}}),
        )
        .await
        .expect("table_create");

    let err = client
        .tools_call(
            "host.state.insert",
            json!({"table": "alerts", "rows": [{"metric": "cpu", "last_value": "high"}]}),
        )
        .await
        .expect_err("wrong-typed value must be rejected");
    assert_eq!(err.error_code.as_deref(), Some("state_schema_violation"));
    assert_eq!(err.data["field"], json!("last_value"));
    assert_eq!(err.data["expected"], json!("real"));

    let result = extract_structured(
        &client
            .tools_call("host.state.query", json!({"table": "alerts"}))
            .await
            .expect("query"),
    );
    assert_eq!(
        result["rows"].as_array().expect("rows array").len(),
        0,
        "the rejected row must not have been written"
    );
}

#[tokio::test]
async fn unknown_column_is_also_rejected() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Monitor").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call(
            "host.state.table_create",
            json!({"name": "alerts", "schema": {"metric": "text"}}),
        )
        .await
        .expect("table_create");

    let err = client
        .tools_call(
            "host.state.insert",
            json!({"table": "alerts", "rows": [{"metric": "cpu", "nope": 1}]}),
        )
        .await
        .expect_err("an undeclared column must be rejected");
    assert_eq!(err.error_code.as_deref(), Some("state_schema_violation"));
    assert_eq!(err.data["field"], json!("nope"));
}
