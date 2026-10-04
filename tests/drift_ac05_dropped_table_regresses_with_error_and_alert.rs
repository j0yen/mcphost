//! PRD-mcphost-drift-review
//! AC5 -- Given a logged query that now errors after a schema change,
//! When reviewed, Then its delta carries `after.error_code`,
//! `regressed_count` is at least 1, and a `drift_regression` alert with
//! severity `warning` exists.
//!
//! The "now errors" schema change here is the table itself being dropped
//! after its note change was queued but before the tick processes it --
//! re-running the logged `SELECT * FROM orders` against a connection
//! where that table no longer exists is an ordinary SQLite "no such
//! table" error, the same structural failure
//! `tables_ac03_cross_tenant_table_name_reads_as_nonexistent`-shaped tests
//! already rely on.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn dropped_table_makes_a_logged_query_error_and_raises_an_alert() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "Drift AC5 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    let tenant = server.state.db.find_tenant_by_namespace(ns).await.expect("find tenant").expect("tenant exists");

    client
        .tools_call("host.table.create", json!({"name": "orders", "columns": {"amount": "real"}}))
        .await
        .expect("create orders");
    client
        .tools_call(
            "host.table.append",
            json!({"table": "orders", "rows": [{"amount": 1.0}, {"amount": 2.0}, {"amount": 3.0}]}),
        )
        .await
        .expect("append");
    client
        .tools_call("host.table.query", json!({"sql": "SELECT * FROM orders"}))
        .await
        .expect("query orders (logs row_count 3, no error)");

    client
        .tools_call(
            "host.table.model_set",
            json!({"table": "orders", "key": "description", "value": "about to be dropped"}),
        )
        .await
        .expect("model_set");

    client
        .tools_call("host.table.drop", json!({"name": "orders", "confirm": true}))
        .await
        .expect("drop orders");

    mcphost::tables_model::tick_once(&server.state).await.expect("tick_once");

    let reviews = extract_structured(
        &client.tools_call("host.drift.reviews", json!({})).await.expect("reviews"),
    );
    let summary = reviews["reviews"]
        .as_array()
        .expect("reviews array")
        .iter()
        .find(|r| r["target"] == "orders" && r["kind"] == "table_note")
        .unwrap_or_else(|| panic!("no table_note review for orders: {reviews}"))
        .clone();
    assert!(
        summary["regressed_count"].as_i64().unwrap_or(0) >= 1,
        "regressed_count must be at least 1: {summary}"
    );
    let item_id = summary["item_id"].as_str().expect("item_id").to_string();

    let review = extract_structured(
        &client.tools_call("host.drift.review", json!({"item_id": item_id})).await.expect("review"),
    );
    let deltas = review["deltas"].as_array().expect("deltas array");
    let regressed_delta = deltas
        .iter()
        .find(|d| d["regressed"] == true)
        .unwrap_or_else(|| panic!("no regressed delta found: {review}"));
    assert!(
        regressed_delta["after"]["error_code"].is_string(),
        "after.error_code must be present: {regressed_delta}"
    );

    let alerts = server.state.db.list_drift_alerts_for_test(tenant.id).await.expect("list_drift_alerts_for_test");
    let drift_alert = alerts
        .iter()
        .find(|a| a.kind == "drift_regression")
        .unwrap_or_else(|| panic!("no drift_regression alert found: {alerts:?}"));
    assert_eq!(drift_alert.severity, "warning", "alert: {drift_alert:?}");
}
