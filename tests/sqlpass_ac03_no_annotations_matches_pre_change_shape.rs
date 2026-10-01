//! PRD-mcphost-table-context-and-sql-passthrough
//! AC3 — Given a table with no annotations, When `host.table.schema` is
//! called, Then the response has no `description` key at any level and
//! matches the pre-change shape byte for byte (`columns` is a plain
//! `{name: type_string}` map, not `{name: {type, description?}}`).

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn schema_with_no_annotations_keeps_scalar_column_shape() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "SqlPass AC3 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call(
            "host.table.create",
            json!({"name": "metrics", "columns": {"metric": "text", "value": "real"}}),
        )
        .await
        .expect("create");

    let schema = extract_structured(
        &client.tools_call("host.table.schema", json!({"table": "metrics"})).await.expect("schema"),
    );
    assert_eq!(
        schema,
        json!({
            "table": "metrics",
            "columns": {"metric": "text", "value": "real"},
            "rows": 0,
            "bytes_used": schema["bytes_used"],
        }),
        "schema with no annotations must match the pre-change shape exactly: {schema}"
    );
    assert!(schema.get("description").is_none());
}
