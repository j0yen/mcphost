//! PRD-mcphost-table-context-and-sql-passthrough
//! AC2 — Given a column with a `description` annotation, When
//! `host.table.schema` is called, Then that column's entry carries the
//! text, and columns without one carry no `description` key.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn column_description_annotation_appears_only_on_that_column() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "SqlPass AC2 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call(
            "host.table.create",
            json!({"name": "expenses", "columns": {"category": "text", "amount": "real"}}),
        )
        .await
        .expect("create expenses");

    client
        .tools_call(
            "host.table.model_set",
            json!({"table": "expenses", "column": "amount", "key": "description", "value": "US dollars."}),
        )
        .await
        .expect("model_set column-level description");

    let schema = extract_structured(
        &client.tools_call("host.table.schema", json!({"table": "expenses"})).await.expect("schema"),
    );
    assert_eq!(schema["columns"]["amount"]["type"], "real", "schema: {schema}");
    assert_eq!(schema["columns"]["amount"]["description"], "US dollars.", "schema: {schema}");
    // `category` has no annotation, so it keeps the pre-change bare-string
    // shape, which has no `description` key to carry.
    assert_eq!(schema["columns"]["category"], "text", "schema: {schema}");
    assert!(
        schema.get("description").is_none(),
        "no annotation was set at table level: {schema}"
    );
}
