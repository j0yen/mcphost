//! PRD-mcphost-table-semantic-model
//! AC9 — Given `host.table.drop`, When `models` lists tables, Then the
//! dropped table's model and annotations are gone.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn drop_removes_the_table_from_models_and_drops_its_annotations() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "TableModel AC9 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call(
            "host.table.create",
            json!({"name": "orders", "columns": {"order_id": "integer"}}),
        )
        .await
        .expect("create orders");
    client
        .tools_call(
            "host.table.append",
            json!({"table": "orders", "rows": [{"order_id": 1}, {"order_id": 2}]}),
        )
        .await
        .expect("append rows");
    client.tools_call("host.table.describe", json!({"table": "orders"})).await.expect("bootstrap describe");
    client
        .tools_call(
            "host.table.model_set",
            json!({"table": "orders", "column": "order_id", "key": "description", "value": "order key"}),
        )
        .await
        .expect("model_set");

    let before = extract_structured(&client.tools_call("host.table.models", json!({})).await.expect("models before drop"));
    let names_before: Vec<&str> = before["tables"].as_array().expect("tables array").iter().map(|t| t["table"].as_str().unwrap()).collect();
    assert!(names_before.contains(&"orders"), "orders must be listed before drop: {before}");

    client.tools_call("host.table.drop", json!({"name": "orders"})).await.expect("drop orders");

    let after = extract_structured(&client.tools_call("host.table.models", json!({})).await.expect("models after drop"));
    let names_after: Vec<&str> = after["tables"].as_array().expect("tables array").iter().map(|t| t["table"].as_str().unwrap()).collect();
    assert!(!names_after.contains(&"orders"), "orders must be gone from models after drop: {after}");

    // Recreating the table under the same name must not resurrect the
    // dropped model's stale annotation or version history.
    client
        .tools_call(
            "host.table.create",
            json!({"name": "orders", "columns": {"order_id": "integer"}}),
        )
        .await
        .expect("recreate orders");
    let recreated = extract_structured(
        &client.tools_call("host.table.describe", json!({"table": "orders"})).await.expect("describe recreated orders"),
    );
    assert_eq!(recreated["version"], 1, "a recreated table must bootstrap a fresh version 1 model: {recreated}");
    assert!(
        recreated["columns"]["order_id"].get("description").is_none(),
        "the dropped table's annotation must not survive under the recreated table: {recreated}"
    );
}
