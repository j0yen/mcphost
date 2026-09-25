//! PRD-mcphost-table-semantic-model
//! AC10 — Given an empty table, When `describe` runs, Then it returns
//! `row_count: 0`, columns with `type: "unknown"`, and no error.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn describe_on_empty_table_reports_unknown_types_without_error() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "TableModel AC10 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call(
            "host.table.create",
            json!({
                "name": "orders",
                "columns": {"order_id": "integer", "status": "text", "created": "timestamp"},
            }),
        )
        .await
        .expect("create orders (never appended to)");

    let result = client.tools_call("host.table.describe", json!({"table": "orders"})).await.expect("describe empty table must not error");
    let model = extract_structured(&result);

    assert_eq!(model["row_count"], 0, "model: {model}");
    for col in ["order_id", "status", "created"] {
        assert_eq!(model["columns"][col]["type"], "unknown", "column {col}: {model}");
    }
    assert_eq!(model["primary_key"], serde_json::Value::Null, "model: {model}");
    assert_eq!(model["foreign_keys"].as_array().unwrap().len(), 0, "model: {model}");
}
