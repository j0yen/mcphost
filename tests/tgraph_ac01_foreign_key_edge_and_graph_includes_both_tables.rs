//! PRD-mcphost-table-concept-graph
//! AC1 — Given tables `orders(id, customer_id, amount, day)` and
//! `customers(id, name, region)` where every `customer_id` exists in
//! `customers.id`, When the tick runs, Then the graph has a `foreign_key`
//! edge `orders.customer_id -> customers.id` with evidence `detected`, and
//! `host.table.graph("orders", 1)` includes both tables.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn orders_customers_graph_has_foreign_key_edge_and_both_tables() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "TGraph AC1 Tenant").await;
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
    // 30 distinct days over 200 rows -- enough that `day` clears the
    // `category` rule's distinct<=20 bound but still parses as an ISO
    // date at a high enough rate to land the `date` role (see
    // tablemodel.rs's role-inference doc comment for the thresholds).
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

    // Bootstraps each table's own model (requirement 3's foreign-key
    // detection needs it), same convention tablemodel_ac02's own fixture
    // uses.
    client.tools_call("host.table.describe", json!({"table": "customers"})).await.expect("describe customers");
    client.tools_call("host.table.describe", json!({"table": "orders"})).await.expect("describe orders");

    let graph = extract_structured(
        &client
            .tools_call("host.table.graph", json!({"table": "orders", "hops": 1}))
            .await
            .expect("host.table.graph"),
    );

    let edges = graph["edges"].as_array().expect("edges array");
    let found = edges.iter().any(|e| {
        e["kind"] == "foreign_key"
            && e["from"] == "orders.customer_id"
            && e["to"] == "customers.id"
            && e["evidence"] == "detected"
    });
    assert!(found, "expected orders.customer_id -> customers.id foreign_key edge (evidence detected): {graph}");

    let node_ids: Vec<&str> = graph["nodes"]
        .as_array()
        .expect("nodes array")
        .iter()
        .filter(|n| n["kind"] == "table")
        .map(|n| n["id"].as_str().expect("node id"))
        .collect();
    assert!(node_ids.contains(&"orders"), "orders table node must be present: {node_ids:?}");
    assert!(node_ids.contains(&"customers"), "customers table node must be present: {node_ids:?}");
}
