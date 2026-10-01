//! PRD-mcphost-table-context-and-sql-passthrough
//! AC2 — Given a column with a `description` annotation, When
//! `host.table.schema` is called, Then that column's entry carries the
//! text, and columns without one carry no `description` key.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn schema_returns_column_level_description_only_for_annotated_column() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "SqlPass AC2 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call(
            "host.table.create",
            json!({"name": "expenses", "columns": {"amount": "real", "category": "text"}}),
        )
        .await
        .expect("create");
    client
        .tools_call(
            "host.table.model_set",
            json!({"table": "expenses", "column": "amount", "key": "description", "value": "amount in USD"}),
        )
        .await
        .expect("model_set column description");

    let schema = extract_structured(
        &client.tools_call("host.table.schema", json!({"table": "expenses"})).await.expect("schema"),
    );
    assert_eq!(
        schema["columns"]["amount"]["description"], "amount in USD",
        "schema: {schema}"
    );
    assert_eq!(schema["columns"]["amount"]["type"], "real", "schema: {schema}");
    assert!(
        schema["columns"]["category"].get("description").is_none(),
        "column without an annotation must carry no description key: {schema}"
    );
    assert_eq!(schema["columns"]["category"]["type"], "text", "schema: {schema}");
    assert!(schema.get("description").is_none(), "no table-level annotation was set: {schema}");
}
