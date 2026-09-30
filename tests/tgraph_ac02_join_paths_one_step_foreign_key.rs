//! PRD-mcphost-table-concept-graph
//! AC2 — Given that graph, When `host.table.join_paths("orders",
//! "customers")` is called, Then the first path has one step, `sql_join`
//! is `orders JOIN customers ON orders.customer_id = customers.id`, and
//! confidence is 1.0.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn join_paths_orders_to_customers_is_one_step_foreign_key_confidence_one() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "TGraph AC2 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call(
            "host.table.create",
            json!({"name": "customers", "columns": {"id": "text", "name": "text", "region": "text"}}),
        )
        .await
        .expect("create customers");
    let customer_ids: Vec<String> = (0..50).map(|i| format!("CUST-{i:04}")).collect();
    let customer_rows: Vec<_> = customer_ids
        .iter()
        .map(|id| json!({"id": id, "name": format!("Customer {id}"), "region": "west"}))
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
                "columns": {"id": "text", "customer_id": "text", "amount": "real", "day": "text"},
            }),
        )
        .await
        .expect("create orders");
    let order_rows: Vec<_> = (0..200)
        .map(|i| {
            json!({
                "id": format!("ORD-{i:04}"),
                "customer_id": customer_ids[i % customer_ids.len()],
                "amount": 5.0 + i as f64,
                "day": format!("2026-01-{:02}", 1 + (i % 30)),
            })
        })
        .collect();
    client
        .tools_call("host.table.append", json!({"table": "orders", "rows": order_rows}))
        .await
        .expect("append orders");

    client.tools_call("host.table.describe", json!({"table": "customers"})).await.expect("describe customers");
    client.tools_call("host.table.describe", json!({"table": "orders"})).await.expect("describe orders");

    let result = extract_structured(
        &client
            .tools_call("host.table.join_paths", json!({"from": "orders", "to": "customers"}))
            .await
            .expect("host.table.join_paths"),
    );

    let paths = result["paths"].as_array().expect("paths array");
    assert!(!paths.is_empty(), "expected at least one join path: {result}");
    let first = &paths[0];
    let steps = first["steps"].as_array().expect("steps array");
    assert_eq!(steps.len(), 1, "expected the first path to have exactly one step: {result}");
    assert_eq!(first["sql_join"], json!("orders JOIN customers ON orders.customer_id = customers.id"), "{result}");
    assert_eq!(first["confidence"], json!(1.0), "{result}");
    assert_eq!(steps[0]["left_table"], json!("orders"));
    assert_eq!(steps[0]["left_column"], json!("customer_id"));
    assert_eq!(steps[0]["right_table"], json!("customers"));
    assert_eq!(steps[0]["right_column"], json!("id"));
    assert_eq!(steps[0]["evidence"], json!("detected"));
}
