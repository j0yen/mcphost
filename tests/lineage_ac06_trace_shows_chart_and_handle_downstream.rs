//! PRD-mcphost-lineage-blast-radius AC6 (P0) -- Given a stored chart and a live handle both derived from
//! `expenses`, When `host.lineage.trace("table:expenses")` is called, Then
//! both appear downstream with their kinds.
//!
//! Unit test of the `lineage::register_edge`/`trace` primitives only: it
//! calls `register_edge` directly rather than going through a real
//! producer's tool path (`handle` has no producer at all yet in mcphost;
//! `chart` does, and is exercised end-to-end by `lineage_ac11`, added
//! after this test's tautological coverage let a real chart-store bug
//! reach prod -- see that file's header for the root cause).

use crate::common;
use common::{TestServer, extract_structured, signup};
use mcphost::lineage::{self, NodeKind};
use serde_json::json;

#[tokio::test]
async fn trace_shows_chart_and_handle_derived_from_the_same_table() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "Lineage AC6 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);
    let tenant = server
        .state
        .db
        .find_tenant_by_namespace(ns)
        .await
        .expect("db")
        .expect("tenant exists");

    lineage::register_edge(
        &server.state,
        tenant.id,
        (NodeKind::Table, "expenses", "expenses"),
        (NodeKind::Chart, "monthly_expenses", "Monthly Expenses"),
        "chart_store",
    )
    .await
    .expect("register chart edge");
    lineage::register_edge(
        &server.state,
        tenant.id,
        (NodeKind::Table, "expenses", "expenses"),
        (NodeKind::Handle, "expenses_live", "Expenses Live"),
        "handle_materialize",
    )
    .await
    .expect("register handle edge");

    let traced = extract_structured(
        &client
            .tools_call("host.lineage.trace", json!({"id": "table:expenses"}))
            .await
            .expect("trace table:expenses"),
    );
    let downstream = traced["downstream"].as_array().expect("downstream array");

    let chart = downstream
        .iter()
        .find(|e| e["id"] == json!("chart:monthly_expenses"))
        .unwrap_or_else(|| panic!("chart not in downstream: {downstream:?}"));
    assert_eq!(chart["kind"], json!("chart"));

    let handle = downstream
        .iter()
        .find(|e| e["id"] == json!("handle:expenses_live"))
        .unwrap_or_else(|| panic!("handle not in downstream: {downstream:?}"));
    assert_eq!(handle["kind"], json!("handle"));
}
