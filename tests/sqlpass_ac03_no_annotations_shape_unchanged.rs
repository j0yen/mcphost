//! PRD-mcphost-table-context-and-sql-passthrough
//! AC3 — Given a table with no annotations, When `host.table.schema` is
//! called, Then the response has no `description` key at any level and
//! matches the pre-change shape byte for byte.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn schema_with_no_annotations_matches_pre_change_shape() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "SqlPass AC3 Tenant").await;
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
            json!({"table": "expenses", "rows": [
                {"category": "produce", "amount": 11.1},
                {"category": "dairy", "amount": 12.21},
            ]}),
        )
        .await
        .expect("append two rows");

    let schema = extract_structured(
        &client.tools_call("host.table.schema", json!({"table": "expenses"})).await.expect("schema"),
    );

    assert!(schema.get("description").is_none(), "no table-level description: {schema}");
    assert!(schema["columns"].get("category_description").is_none());
    let obj = schema.as_object().expect("schema is an object");
    assert_eq!(
        obj.keys().cloned().collect::<std::collections::BTreeSet<_>>(),
        ["table", "columns", "rows", "bytes_used"].into_iter().map(str::to_string).collect(),
        "no annotations: the response must have exactly the pre-change top-level keys: {schema}"
    );
    assert_eq!(schema["table"], "expenses");
    assert_eq!(schema["rows"], 2);
    assert_eq!(schema["columns"]["category"], "text", "bare type string, no shape change: {schema}");
    assert_eq!(schema["columns"]["amount"], "real", "bare type string, no shape change: {schema}");
    assert!(schema["bytes_used"].as_i64().unwrap() > 0);
}
