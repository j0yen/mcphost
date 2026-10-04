//! PRD-mcphost-drift-review
//! AC4 -- Given an append that changes a column's inferred role, When the
//! review runs, Then a logged GROUP BY over that column shows
//! `before.row_count` 7 and `after.row_count` 31 with `changed: true`.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn append_shifts_group_by_row_count_from_7_to_31() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "Drift AC4 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    server.state.db.find_tenant_by_namespace(ns).await.expect("find tenant").expect("tenant exists");

    client
        .tools_call("host.table.create", json!({"name": "metrics", "columns": {"day": "text"}}))
        .await
        .expect("create metrics");

    let first_rows: Vec<_> = (0..7).map(|i| json!({"day": format!("d{i}")})).collect();
    client
        .tools_call("host.table.append", json!({"table": "metrics", "rows": first_rows}))
        .await
        .expect("append 7 rows");

    // Bootstraps a non-stale model v1 synchronously -- no drift version/
    // queue entry yet (the state right before the GROUP BY below is
    // logged).
    client.tools_call("host.table.describe", json!({"table": "metrics"})).await.expect("describe (bootstrap)");

    let before = extract_structured(
        &client
            .tools_call("host.table.query", json!({"sql": "SELECT day, COUNT(*) AS n FROM metrics GROUP BY day"}))
            .await
            .expect("group by query"),
    );
    assert_eq!(before["rows"].as_array().expect("rows").len(), 7, "before: {before}");

    let more_rows: Vec<_> = (7..31).map(|i| json!({"day": format!("d{i}")})).collect();
    client
        .tools_call("host.table.append", json!({"table": "metrics", "rows": more_rows}))
        .await
        .expect("append 24 more rows");

    mcphost::tables_model::tick_once(&server.state).await.expect("tick_once");

    let reviews = extract_structured(
        &client.tools_call("host.drift.reviews", json!({})).await.expect("reviews"),
    );
    let summary = reviews["reviews"]
        .as_array()
        .expect("reviews array")
        .iter()
        .find(|r| r["target"] == "metrics" && r["kind"] == "table_schema")
        .unwrap_or_else(|| panic!("no table_schema review for metrics: {reviews}"))
        .clone();
    let item_id = summary["item_id"].as_str().expect("item_id").to_string();

    let review = extract_structured(
        &client.tools_call("host.drift.review", json!({"item_id": item_id})).await.expect("review"),
    );
    let deltas = review["deltas"].as_array().expect("deltas array");
    let group_by_delta = deltas
        .iter()
        .find(|d| d["sql"].as_str().unwrap_or_default().contains("GROUP BY"))
        .unwrap_or_else(|| panic!("no GROUP BY delta found: {review}"));
    assert_eq!(group_by_delta["before"]["row_count"], 7, "delta: {group_by_delta}");
    assert_eq!(group_by_delta["after"]["row_count"], 31, "delta: {group_by_delta}");
    assert_eq!(group_by_delta["changed"], true, "delta: {group_by_delta}");
}
