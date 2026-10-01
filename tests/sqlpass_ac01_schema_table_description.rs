//! PRD-mcphost-table-context-and-sql-passthrough
//! AC1 — Given a declared table with a table-level `description`
//! annotation set through `host.table.model_set`, When `host.table.schema`
//! is called, Then the response carries that text under `description` and
//! the `table`, `columns`, `rows`, `bytes_used` keys are unchanged.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn schema_returns_table_level_description() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "SqlPass AC1 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call(
            "host.table.create",
            json!({"name": "expenses", "columns": {"amount": "real"}}),
        )
        .await
        .expect("create");
    client
        .tools_call(
            "host.table.model_set",
            json!({"table": "expenses", "key": "description", "value": "Monthly expense ledger"}),
        )
        .await
        .expect("model_set table description");

    let schema = extract_structured(
        &client.tools_call("host.table.schema", json!({"table": "expenses"})).await.expect("schema"),
    );
    assert_eq!(schema["description"], "Monthly expense ledger", "schema: {schema}");
    assert_eq!(schema["table"], "expenses", "schema: {schema}");
    assert_eq!(schema["rows"], 0, "schema: {schema}");
    assert!(schema["bytes_used"].as_i64().unwrap_or(0) > 0, "schema: {schema}");
}
