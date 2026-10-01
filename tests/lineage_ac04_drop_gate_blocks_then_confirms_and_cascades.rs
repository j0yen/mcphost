//! PRD-mcphost-lineage-blast-radius AC4 (P0) -- Given a table with a breaking chain and tool consumer, When
//! `host.table.drop("orders")` is called without `confirm`, Then
//! `lineage_blocked` lists the chain and the tool with their uses and the
//! table still exists; When called with `confirm: true`, Then the table is
//! dropped, its nodes and edges are removed, and dependent chart and handle
//! nodes are marked `orphaned`.

use crate::common;
use common::{TestServer, chain_kind_registry, extract_structured, publish, signup};
use mcphost::lineage::{self, NodeKind};
use serde_json::json;

#[tokio::test]
async fn drop_is_blocked_then_confirmed_and_cascades() {
    let server = TestServer::start_with_kinds(chain_kind_registry()).await;
    let (ns, key) = signup(&server.base_url, "Lineage AC4 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);
    let tenant = server
        .state
        .db
        .find_tenant_by_namespace(ns)
        .await
        .expect("db")
        .expect("tenant exists");

    client
        .tools_call("host.table.create", json!({"name": "orders", "columns": {"id": "integer"}}))
        .await
        .expect("create orders");

    let schema = json!({"type": "object"});
    publish(&client, "orders_step", "echo", json!({"schema": schema, "reads": ["orders"]})).await;
    publish(&client, "orders_reader", "echo", json!({"schema": schema, "reads": ["orders"]})).await;
    publish(
        &client,
        "pipeline",
        "chain",
        json!({"steps": [{"tool": "orders_step", "args": {}}]}),
    )
    .await;

    // A chart and a handle derived from `orders` -- registered directly
    // through the lineage module's own API, the same integration point a
    // future chart-store/live-handle PRD's own publish path would call
    // (requirement 4; neither feature exists in mcphost yet).
    lineage::register_edge(
        &server.state,
        tenant.id,
        (NodeKind::Table, "orders", "orders"),
        (NodeKind::Chart, "weekly_orders", "Weekly Orders"),
        "chart_store",
    )
    .await
    .expect("register chart edge");
    lineage::register_edge(
        &server.state,
        tenant.id,
        (NodeKind::Table, "orders", "orders"),
        (NodeKind::Handle, "orders_live", "Orders Live"),
        "handle_materialize",
    )
    .await
    .expect("register handle edge");

    let err = client
        .tools_call("host.table.drop", json!({"name": "orders"}))
        .await
        .expect_err("a breaking drop without confirm must be refused");
    assert_eq!(err.error_code.as_deref(), Some("lineage_blocked"));
    let blocked = err.data["lineage_blocked"].as_array().expect("lineage_blocked array");
    let blocked_ids: Vec<&str> = blocked.iter().map(|c| c["id"].as_str().unwrap()).collect();
    assert!(blocked_ids.contains(&"chain:pipeline"), "blocked: {blocked:?}");
    assert!(blocked_ids.contains(&"tool:orders_reader"), "blocked: {blocked:?}");
    for c in blocked {
        assert!(c["uses"].is_number(), "consumer must carry uses: {c:?}");
    }

    // The table still exists.
    let schema_resp = client
        .tools_call("host.table.schema", json!({"table": "orders"}))
        .await
        .expect("orders must still exist after the refusal");
    assert_eq!(extract_structured(&schema_resp)["table"], json!("orders"));

    let confirmed = client
        .tools_call("host.table.drop", json!({"name": "orders", "confirm": true}))
        .await
        .expect("confirmed drop must succeed despite breaking consumers");
    assert_eq!(extract_structured(&confirmed)["dropped"], json!(true));

    client
        .tools_call("host.table.schema", json!({"table": "orders"}))
        .await
        .expect_err("orders must no longer exist after the confirmed drop");

    let traced_table = extract_structured(
        &client
            .tools_call("host.lineage.trace", json!({"id": "table:orders"}))
            .await
            .expect("trace table:orders"),
    );
    assert!(
        traced_table.get("kind").is_none(),
        "table:orders' own node must be removed: {traced_table:?}"
    );
    assert_eq!(
        traced_table["downstream"],
        json!([]),
        "table:orders' edges must be removed: {traced_table:?}"
    );

    let traced_chart = extract_structured(
        &client
            .tools_call("host.lineage.trace", json!({"id": "chart:weekly_orders"}))
            .await
            .expect("trace chart:weekly_orders"),
    );
    assert_eq!(traced_chart["orphaned"], json!(true), "chart: {traced_chart:?}");

    let traced_handle = extract_structured(
        &client
            .tools_call("host.lineage.trace", json!({"id": "handle:orders_live"}))
            .await
            .expect("trace handle:orders_live"),
    );
    assert_eq!(traced_handle["orphaned"], json!(true), "handle: {traced_handle:?}");
}
