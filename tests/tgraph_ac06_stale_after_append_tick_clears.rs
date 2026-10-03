//! PRD-mcphost-table-concept-graph
//! AC6 — Given a `host.table.append` to `orders`, When `graph` is called
//! within 30 s, Then `stale` is true until the tick rebuilds, and false
//! after.
//!
//! Drives `tables_model::tick_once` directly rather than waiting on the
//! real 10s background cadence (which internally calls
//! `tables_graph::tick_once` at its own tail) -- same deterministic-tick
//! convention `tablemodel_ac05_stale_after_append_recomputes_on_tick.rs`
//! already uses for `table_models`' own staleness.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn graph_is_stale_after_append_and_clears_after_tick() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "TGraph AC6 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call(
            "host.table.create",
            json!({"name": "customers", "columns": {"id": "text", "name": "text"}}),
        )
        .await
        .expect("create customers");
    let customer_ids: Vec<String> = (0..20).map(|i| format!("CUST-{i:04}")).collect();
    let customer_rows: Vec<_> =
        customer_ids.iter().map(|id| json!({"id": id, "name": format!("Customer {id}")})).collect();
    client
        .tools_call("host.table.append", json!({"table": "customers", "rows": customer_rows}))
        .await
        .expect("append customers");

    client
        .tools_call(
            "host.table.create",
            json!({"name": "orders", "columns": {"id": "text", "customer_id": "text", "amount": "real"}}),
        )
        .await
        .expect("create orders");
    let order_rows: Vec<_> = (0..100)
        .map(|i| {
            json!({
                "id": format!("ORD-{i:04}"),
                "customer_id": customer_ids[i % customer_ids.len()],
                "amount": 5.0 + i as f64,
            })
        })
        .collect();
    client.tools_call("host.table.append", json!({"table": "orders", "rows": order_rows})).await.expect("append orders");

    client.tools_call("host.table.describe", json!({"table": "customers"})).await.expect("describe customers");
    client.tools_call("host.table.describe", json!({"table": "orders"})).await.expect("describe orders");

    let first = extract_structured(
        &client.tools_call("host.table.graph", json!({"table": "orders", "hops": 1})).await.expect("graph (bootstrap)"),
    );
    assert_eq!(first["stale"], false, "a freshly bootstrapped graph must not be stale: {first}");
    let first_version = first["version"].as_i64().expect("version number");

    let more_rows: Vec<_> = (100..150)
        .map(|i| {
            json!({
                "id": format!("ORD-{i:04}"),
                "customer_id": customer_ids[i % customer_ids.len()],
                "amount": 5.0 + i as f64,
            })
        })
        .collect();
    client.tools_call("host.table.append", json!({"table": "orders", "rows": more_rows})).await.expect("append more orders");

    let right_after = extract_structured(
        &client
            .tools_call("host.table.graph", json!({"table": "orders", "hops": 1}))
            .await
            .expect("graph (right after append)"),
    );
    assert_eq!(right_after["stale"], true, "graph: {right_after}");
    assert_eq!(right_after["version"], first_version, "graph: {right_after}");

    mcphost::tables_model::tick_once(&server.state).await.expect("tick_once");

    let after_tick = extract_structured(
        &client.tools_call("host.table.graph", json!({"table": "orders", "hops": 1})).await.expect("graph (after tick)"),
    );
    assert_eq!(after_tick["stale"], false, "graph: {after_tick}");
    assert!(
        after_tick["version"].as_i64().unwrap() > first_version,
        "tick's rebuild must bump the version: {after_tick}"
    );
}
