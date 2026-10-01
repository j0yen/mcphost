//! PRD-mcphost-lineage-blast-radius AC9 (P1) -- Given a `model_set` role change on a column a chart uses,
//! When applied, Then the response includes a `degrading` note naming the
//! chart and the change is applied.

use crate::common;
use common::{TestServer, extract_structured, signup};
use mcphost::lineage::{self, NodeKind};
use serde_json::json;

#[tokio::test]
async fn role_change_reports_a_degrading_note_for_the_chart_and_still_applies() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "Lineage AC9 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);
    let tenant = server
        .state
        .db
        .find_tenant_by_namespace(ns)
        .await
        .expect("db")
        .expect("tenant exists");

    client
        .tools_call(
            "host.table.create",
            json!({"name": "orders", "columns": {"id": "integer", "amount": "real"}}),
        )
        .await
        .expect("create orders");

    lineage::register_edge(
        &server.state,
        tenant.id,
        (NodeKind::Table, "orders", "orders"),
        (NodeKind::Chart, "revenue_chart", "Revenue Chart"),
        "chart_store",
    )
    .await
    .expect("register chart edge");

    let resp = extract_structured(
        &client
            .tools_call(
                "host.table.model_set",
                json!({"table": "orders", "column": "amount", "key": "role", "value": "measure"}),
            )
            .await
            .expect("model_set"),
    );

    assert_eq!(resp["set"], json!(true), "the annotation must still be applied: {resp:?}");
    let notes = resp["lineage_notes"].as_array().expect("lineage_notes array");
    let chart_note = notes
        .iter()
        .find(|n| n["id"] == json!("chart:revenue_chart"))
        .unwrap_or_else(|| panic!("chart:revenue_chart not in lineage_notes: {notes:?}"));
    assert_eq!(chart_note["severity"], json!("degrading"), "note: {chart_note:?}");

    // The change really was applied, not just reported.
    let described = extract_structured(
        &client
            .tools_call("host.table.describe", json!({"table": "orders"}))
            .await
            .expect("describe"),
    );
    assert_eq!(described["columns"]["amount"]["role"], json!("measure"), "described: {described:?}");
}
