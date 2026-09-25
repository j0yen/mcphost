//! PRD-mcphost-table-semantic-model
//! AC8 — Given `model_set {column: "customer_id", key: "role", value:
//! "text"}`, When `describe` runs, Then `role` is `text` and
//! `inferred_role` still shows `id`.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn model_set_role_override_keeps_inferred_role_visible() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "TableModel AC8 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call(
            "host.table.create",
            json!({"name": "orders", "columns": {"order_id": "integer", "customer_id": "text"}}),
        )
        .await
        .expect("create orders");

    // `customer_id`: 480 distinct fixed-width values across 500 rows
    // (some repeat) -- lands on the `id` rule (same construction AC1 uses).
    let rows: Vec<_> = (0..500)
        .map(|i| json!({"order_id": i, "customer_id": format!("CUST-{:04}", i % 480)}))
        .collect();
    client
        .tools_call("host.table.append", json!({"table": "orders", "rows": rows}))
        .await
        .expect("append 500 rows");

    let before = extract_structured(
        &client.tools_call("host.table.describe", json!({"table": "orders"})).await.expect("describe (bootstrap)"),
    );
    assert_eq!(before["columns"]["customer_id"]["role"], "id", "model: {before}");
    assert_eq!(before["columns"]["customer_id"]["inferred_role"], "id", "model: {before}");

    client
        .tools_call(
            "host.table.model_set",
            json!({"table": "orders", "column": "customer_id", "key": "role", "value": "text"}),
        )
        .await
        .expect("model_set role override");

    let after = extract_structured(
        &client.tools_call("host.table.describe", json!({"table": "orders"})).await.expect("describe (after override)"),
    );
    assert_eq!(after["columns"]["customer_id"]["role"], "text", "model: {after}");
    assert_eq!(
        after["columns"]["customer_id"]["inferred_role"], "id",
        "inferred_role must keep showing the raw inference: {after}"
    );
}
