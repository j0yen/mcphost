//! PRD-mcphost-table-concept-graph
//! AC7 — Given two tenants with identically named tables, When tenant A
//! calls any of the three tools, Then no node or edge from tenant B
//! appears.
//!
//! Tenant B's `orders`/`customers` carry one extra column each
//! (`secret_field`/`other_field`) beyond tenant A's own schema -- a clean,
//! unambiguous leak signal: those column nodes can only show up in A's
//! graph if B's own model data crossed the tenant boundary, since A's own
//! `host.table.create` never declared them.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

async fn build_orders_customers(client: &McpClient, extra_order_col: bool, extra_customer_col: bool) {
    let mut customer_cols = json!({"id": "text", "name": "text"});
    if extra_customer_col {
        customer_cols["other_field"] = json!("text");
    }
    client.tools_call("host.table.create", json!({"name": "customers", "columns": customer_cols})).await.expect("create customers");
    let customer_ids: Vec<String> = (0..20).map(|i| format!("CUST-{i:04}")).collect();
    let customer_rows: Vec<_> = customer_ids
        .iter()
        .map(|id| {
            let mut row = json!({"id": id, "name": format!("Customer {id}")});
            if extra_customer_col {
                row["other_field"] = json!("x");
            }
            row
        })
        .collect();
    client.tools_call("host.table.append", json!({"table": "customers", "rows": customer_rows})).await.expect("append customers");

    let mut order_cols = json!({"id": "text", "customer_id": "text", "amount": "real"});
    if extra_order_col {
        order_cols["secret_field"] = json!("text");
    }
    client.tools_call("host.table.create", json!({"name": "orders", "columns": order_cols})).await.expect("create orders");
    let order_rows: Vec<_> = (0..100)
        .map(|i| {
            let mut row = json!({
                "id": format!("ORD-{i:04}"),
                "customer_id": customer_ids[i % customer_ids.len()],
                "amount": 5.0 + i as f64,
            });
            if extra_order_col {
                row["secret_field"] = json!("y");
            }
            row
        })
        .collect();
    client.tools_call("host.table.append", json!({"table": "orders", "rows": order_rows})).await.expect("append orders");

    client.tools_call("host.table.describe", json!({"table": "customers"})).await.expect("describe customers");
    client.tools_call("host.table.describe", json!({"table": "orders"})).await.expect("describe orders");
}

#[tokio::test]
async fn tenant_a_graph_join_paths_and_next_questions_never_see_tenant_b() {
    let server = TestServer::start().await;

    let (_ns_a, key_a) = signup(&server.base_url, "TGraph AC7 Tenant A").await;
    let client_a = McpClient::with_bearer(&server.base_url, &key_a);
    build_orders_customers(&client_a, false, false).await;

    let (_ns_b, key_b) = signup(&server.base_url, "TGraph AC7 Tenant B").await;
    let client_b = McpClient::with_bearer(&server.base_url, &key_b);
    build_orders_customers(&client_b, true, true).await;

    let graph = extract_structured(&client_a.tools_call("host.table.graph", json!({})).await.expect("A graph"));
    let node_ids: Vec<&str> =
        graph["nodes"].as_array().expect("nodes array").iter().map(|n| n["id"].as_str().expect("node id")).collect();
    assert!(node_ids.contains(&"orders.id"), "A's own orders.id must be present: {node_ids:?}");
    assert!(node_ids.contains(&"customers.id"), "A's own customers.id must be present: {node_ids:?}");
    assert!(
        !node_ids.contains(&"orders.secret_field"),
        "tenant B's orders.secret_field must never appear in A's graph: {node_ids:?}"
    );
    assert!(
        !node_ids.contains(&"customers.other_field"),
        "tenant B's customers.other_field must never appear in A's graph: {node_ids:?}"
    );

    let join = extract_structured(
        &client_a
            .tools_call("host.table.join_paths", json!({"from": "orders", "to": "customers"}))
            .await
            .expect("A join_paths"),
    );
    let paths = join["paths"].as_array().expect("paths array");
    assert!(!paths.is_empty(), "{join}");
    // `orders`/`customers` both also declare a same-type `id` column, so A's
    // own fixture legitimately has a second (same_name, confidence 0.5)
    // path alongside the foreign-key one -- unrelated to B. The foreign-key
    // path must still sort first (confidence 1.0 beats 0.5 at equal
    // length), and no path may involve a column only B declared.
    let first = &paths[0];
    assert_eq!(first["confidence"], json!(1.0), "{join}");
    for path in paths {
        for step in path["steps"].as_array().expect("steps array") {
            let left = step["left_column"].as_str().expect("left_column");
            let right = step["right_column"].as_str().expect("right_column");
            assert!(left != "secret_field" && left != "other_field", "{join}");
            assert!(right != "secret_field" && right != "other_field", "{join}");
        }
    }

    let questions = extract_structured(
        &client_a.tools_call("host.table.next_questions", json!({"table": "orders"})).await.expect("A next_questions"),
    );
    for q in questions["questions"].as_array().expect("questions array") {
        let sql = q["sql"].as_str().expect("sql string");
        assert!(!sql.contains("secret_field"), "A's next_questions must never reference B's column: {sql}");
    }
}
