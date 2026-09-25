//! PRD-mcphost-table-semantic-model
//! AC2 — Given a `customers` table whose `id` is a key and
//! `orders.customer_id` values all appear in it, When
//! `describe {table: "orders"}` runs, Then `foreign_keys` contains
//! `customer_id -> customers.id`.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn describe_detects_foreign_key_to_customers_id() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "TableModel AC2 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call(
            "host.table.create",
            json!({"name": "customers", "columns": {"id": "text", "name": "text"}}),
        )
        .await
        .expect("create customers");
    let customer_ids: Vec<String> = (0..50).map(|i| format!("CUST-{i:04}")).collect();
    let customer_rows: Vec<_> = customer_ids
        .iter()
        .map(|id| json!({"id": id, "name": format!("Customer {id}")}))
        .collect();
    client
        .tools_call("host.table.append", json!({"table": "customers", "rows": customer_rows}))
        .await
        .expect("append customers");

    client
        .tools_call(
            "host.table.create",
            json!({
                "name": "orders",
                "columns": {"order_id": "integer", "customer_id": "text", "amount": "real"},
            }),
        )
        .await
        .expect("create orders");
    let order_rows: Vec<_> = (0..200)
        .map(|i| {
            json!({
                "order_id": i,
                "customer_id": customer_ids[i % customer_ids.len()],
                "amount": 5.0 + i as f64,
            })
        })
        .collect();
    client
        .tools_call("host.table.append", json!({"table": "orders", "rows": order_rows}))
        .await
        .expect("append orders");

    let result = client
        .tools_call("host.table.describe", json!({"table": "orders"}))
        .await
        .expect("describe orders");
    let model = extract_structured(&result);

    let fks = model["foreign_keys"].as_array().expect("foreign_keys array");
    let found = fks.iter().any(|fk| {
        fk["column"] == "customer_id" && fk["references_table"] == "customers" && fk["references_column"] == "id"
    });
    assert!(found, "expected customer_id -> customers.id in foreign_keys: {model}");
}
