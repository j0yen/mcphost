//! PRD-mcphost-drift-review
//! AC2 -- Given 12 logged queries naming `expenses` and a note change on
//! it, When the review is produced, Then it holds 12 deltas,
//! `changed_count` 0, `regressed_count` 0, and no alert is written.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn cosmetic_note_change_produces_12_unchanged_deltas_no_alert() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "Drift AC2 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    let tenant = server.state.db.find_tenant_by_namespace(ns).await.expect("find tenant").expect("tenant exists");

    client
        .tools_call(
            "host.table.create",
            json!({"name": "expenses", "columns": {"amount": "real", "category": "text"}}),
        )
        .await
        .expect("create expenses");
    client
        .tools_call(
            "host.table.append",
            json!({"table": "expenses", "rows": [
                {"amount": 10.0, "category": "food"},
                {"amount": 20.0, "category": "rent"},
            ]}),
        )
        .await
        .expect("append");

    for i in 0..12 {
        client
            .tools_call("host.table.query", json!({"sql": format!("SELECT * FROM expenses WHERE amount > {i}")}))
            .await
            .unwrap_or_else(|e| panic!("query {i} failed: {} {}", e.code, e.message));
    }

    client
        .tools_call(
            "host.table.model_set",
            json!({"table": "expenses", "key": "description", "value": "USD -> EUR, cosmetic only"}),
        )
        .await
        .expect("model_set");

    mcphost::tables_model::tick_once(&server.state).await.expect("tick_once");

    let reviews = extract_structured(
        &client.tools_call("host.drift.reviews", json!({"open_only": true})).await.expect("reviews"),
    );
    let items = reviews["reviews"].as_array().expect("reviews array");
    let summary = items
        .iter()
        .find(|r| r["target"] == "expenses" && r["kind"] == "table_note")
        .unwrap_or_else(|| panic!("no table_note review for expenses: {items:?}"));
    let item_id = summary["item_id"].as_str().expect("item_id").to_string();

    let review = extract_structured(
        &client.tools_call("host.drift.review", json!({"item_id": item_id})).await.expect("review"),
    );
    let deltas = review["deltas"].as_array().expect("deltas array");
    assert_eq!(deltas.len(), 12, "expected 12 deltas: {review}");
    assert_eq!(review["changed_count"], 0, "review: {review}");
    assert_eq!(review["regressed_count"], 0, "review: {review}");

    let alerts = server.state.db.list_drift_alerts_for_test(tenant.id).await.expect("list_drift_alerts_for_test");
    assert!(alerts.is_empty(), "no alert should be written for a cosmetic change: {alerts:?}");
}
