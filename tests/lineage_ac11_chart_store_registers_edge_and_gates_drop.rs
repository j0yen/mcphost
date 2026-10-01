//! PRD-mcphost-lineage-blast-radius AC11 (P0, prod regression) -- Given a
//! fresh tenant stores a chart over `expenses` and calls
//! `host.table.drop("expenses")` without `confirm`, Then `lineage_blocked`
//! names the chart; with `confirm: true` the drop succeeds. Unlike
//! `lineage_ac06` (which calls `lineage::register_edge` directly, a unit
//! test of the primitive only), this test goes through the REAL tool path:
//! `host.table.chart(..., share: true)` -> `host.lineage.trace` ->
//! `host.table.drop`.
//!
//! Prod found this (alarm `live-ac-failed` on `mcphost-lineage-blast-
//! radius`, v0.64.0): `src/chart.rs`'s `table_chart` never called
//! `lineage::register_edge` when it stored a chart, and `src/handler.rs`
//! dispatches `host.table.chart` with no lineage wrapper, so a stored
//! chart was invisible to the drop gate. `lineage_ac06`'s direct-call
//! fixture passed throughout because it never exercised the chart-store
//! code path at all. Fixed by registering the edge from the SQL's base
//! table to the stored chart right after `store_chart` succeeds (same
//! best-effort convention `control::tool_publish`'s own lineage
//! registration already uses for tool/chain publish).

use crate::chart_fixture;
use crate::common;
use common::{TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn chart_store_registers_edge_and_gates_the_source_table_drop() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Lineage AC11 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    chart_fixture::seed_expenses(&client).await;

    let chart = extract_structured(
        &client
            .tools_call(
                "host.table.chart",
                json!({
                    "sql": "SELECT category, SUM(amount) AS total FROM expenses GROUP BY category",
                    "title": "Expenses by category",
                    "share": true,
                }),
            )
            .await
            .expect("host.table.chart share:true"),
    );
    let chart_id = chart["chart_id"].as_str().expect("chart_id present").to_string();

    // AC11 part 1: `host.lineage.trace` shows the stored chart downstream
    // of `expenses` -- registered by the real store path, not a direct
    // `register_edge` call.
    let traced = extract_structured(
        &client
            .tools_call("host.lineage.trace", json!({"id": "table:expenses"}))
            .await
            .expect("trace table:expenses"),
    );
    let downstream = traced["downstream"].as_array().expect("downstream array");
    let edge = downstream
        .iter()
        .find(|e| e["id"] == json!(format!("chart:{chart_id}")))
        .unwrap_or_else(|| panic!("chart:{chart_id} not in downstream: {downstream:?}"));
    assert_eq!(edge["kind"], json!("chart"));
    assert_eq!(edge["evidence"], json!("chart_store"), "edge: {edge:?}");

    // AC11 part 2: an unconfirmed drop is refused, naming the chart.
    let err = client
        .tools_call("host.table.drop", json!({"name": "expenses"}))
        .await
        .expect_err("a drop with a breaking chart consumer must be refused");
    assert_eq!(err.error_code.as_deref(), Some("lineage_blocked"));
    let blocked = err.data["lineage_blocked"].as_array().expect("lineage_blocked array");
    assert!(
        blocked.iter().any(|c| c["id"] == json!(format!("chart:{chart_id}"))),
        "blocked: {blocked:?}"
    );

    // AC11 part 3: `confirm: true` succeeds despite the breaking chart.
    let confirmed = client
        .tools_call("host.table.drop", json!({"name": "expenses", "confirm": true}))
        .await
        .expect("confirmed drop must succeed despite the breaking chart");
    assert_eq!(extract_structured(&confirmed)["dropped"], json!(true));
}
