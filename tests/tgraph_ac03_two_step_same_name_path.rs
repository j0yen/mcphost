//! PRD-mcphost-table-concept-graph
//! AC3 — Given a third table `regions(region, manager)` with no foreign key
//! but a `region` column of the same type as `customers.region`, When
//! `join_paths("orders", "regions")` is called, Then a two-step path
//! returns via `same_name` with confidence 0.5 and evidence per step.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn join_paths_orders_to_regions_is_two_step_via_same_name_confidence_half() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "TGraph AC3 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call(
            "host.table.create",
            json!({"name": "customers", "columns": {"id": "text", "name": "text", "region": "text"}}),
        )
        .await
        .expect("create customers");
    let customer_ids: Vec<String> = (0..50).map(|i| format!("CUST-{i:04}")).collect();
    // Every customer's own `region` is the same constant value -- distinct
    // from every value `regions.region` holds below, so the `region`
    // column never accidentally clears requirement 3's foreign-key
    // coverage bound (95% of a candidate column's own values found in a
    // sibling's key column): the same_name edge this AC tests for must
    // come from the name+type match alone, not from a foreign key this
    // fixture doesn't intend.
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

    client
        .tools_call(
            "host.table.create",
            json!({"name": "regions", "columns": {"region": "text", "manager": "text"}}),
        )
        .await
        .expect("create regions");
    let region_rows: Vec<_> = ["east", "north", "south", "central", "pacific"]
        .iter()
        .map(|r| json!({"region": r, "manager": format!("Manager {r}")}))
        .collect();
    client
        .tools_call("host.table.append", json!({"table": "regions", "rows": region_rows}))
        .await
        .expect("append regions");

    client.tools_call("host.table.describe", json!({"table": "customers"})).await.expect("describe customers");
    client.tools_call("host.table.describe", json!({"table": "orders"})).await.expect("describe orders");
    client.tools_call("host.table.describe", json!({"table": "regions"})).await.expect("describe regions");

    // No foreign key should have been detected into/out of regions --
    // guards the fixture itself against the very coverage accident the
    // comment above explains.
    let regions_model = extract_structured(
        &client.tools_call("host.table.describe", json!({"table": "regions"})).await.expect("describe regions again"),
    );
    assert_eq!(
        regions_model["foreign_keys"].as_array().expect("foreign_keys array").len(),
        0,
        "regions must have no detected foreign key in this fixture: {regions_model}"
    );

    let result = extract_structured(
        &client
            .tools_call("host.table.join_paths", json!({"from": "orders", "to": "regions"}))
            .await
            .expect("host.table.join_paths"),
    );

    let paths = result["paths"].as_array().expect("paths array");
    assert!(!paths.is_empty(), "expected at least one join path: {result}");
    let first = &paths[0];
    let steps = first["steps"].as_array().expect("steps array");
    assert_eq!(steps.len(), 2, "expected a two-step path: {result}");
    assert_eq!(first["confidence"], json!(0.5), "{result}");
    assert!(steps.iter().all(|s| s["evidence"].is_string()), "every step must carry its own evidence: {result}");
    assert_eq!(
        steps[1]["evidence"], json!("same_name"),
        "the second (customers -> regions) step must be evidenced by same_name: {result}"
    );
    assert_eq!(steps[1]["left_table"], json!("customers"));
    assert_eq!(steps[1]["right_table"], json!("regions"));
}
