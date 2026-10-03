//! PRD-mcphost-table-context-and-sql-passthrough
//! AC1 — Given a declared table with a table-level `description`
//! annotation set through `host.table.model_set`, When `host.table.schema`
//! is called, Then the response carries that text under `description` and
//! the `table`, `columns`, `rows`, `bytes_used` keys are unchanged.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn table_level_description_annotation_appears_in_schema() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "SqlPass AC1 Tenant").await;
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
            "host.table.append",
            json!({"table": "expenses", "rows": [{"category": "produce", "amount": 11.1}]}),
        )
        .await
        .expect("append one row");

    let before = extract_structured(
        &client.tools_call("host.table.schema", json!({"table": "expenses"})).await.expect("schema before annotation"),
    );
    assert!(before.get("description").is_none(), "no description yet: {before}");

    client
        .tools_call(
            "host.table.model_set",
            json!({"table": "expenses", "key": "description", "value": "Grocery spending by category."}),
        )
        .await
        .expect("model_set table-level description");

    let after = extract_structured(
        &client.tools_call("host.table.schema", json!({"table": "expenses"})).await.expect("schema after annotation"),
    );
    assert_eq!(after["description"], "Grocery spending by category.", "schema: {after}");
    assert_eq!(after["table"], "expenses", "schema: {after}");
    assert_eq!(after["rows"], 1, "schema: {after}");
    assert!(after["bytes_used"].as_i64().unwrap() > 0, "schema: {after}");
    assert_eq!(after["columns"]["category"], "text", "table-level description must not change column shape: {after}");
    assert_eq!(after["columns"]["amount"], "real", "table-level description must not change column shape: {after}");
}
