//! PRD-mcphost-drift-review
//! AC10 -- Given `host.drift.check("expenses")` with no version change,
//! When the tick runs, Then a review with the current deltas is
//! produced.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn manual_check_produces_a_review_with_current_deltas() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Drift AC10 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call("host.table.create", json!({"name": "expenses", "columns": {"amount": "real"}}))
        .await
        .expect("create expenses");
    client
        .tools_call("host.table.append", json!({"table": "expenses", "rows": [{"amount": 1.0}, {"amount": 2.0}]}))
        .await
        .expect("append");
    client
        .tools_call("host.table.query", json!({"sql": "SELECT * FROM expenses"}))
        .await
        .expect("query");

    let check = extract_structured(
        &client.tools_call("host.drift.check", json!({"target": "expenses"})).await.expect("check"),
    );
    assert_eq!(check["queued"], true, "check: {check}");
    assert_eq!(check["target"], "expenses", "check: {check}");

    mcphost::tables_model::tick_once(&server.state).await.expect("tick_once");

    let reviews = extract_structured(
        &client.tools_call("host.drift.reviews", json!({})).await.expect("reviews"),
    );
    let summary = reviews["reviews"]
        .as_array()
        .expect("reviews array")
        .iter()
        .find(|r| r["target"] == "expenses")
        .unwrap_or_else(|| panic!("no review produced for a manual check with no version change: {reviews}"))
        .clone();
    let item_id = summary["item_id"].as_str().expect("item_id").to_string();

    let review = extract_structured(
        &client.tools_call("host.drift.review", json!({"item_id": item_id})).await.expect("review"),
    );
    let deltas = review["deltas"].as_array().expect("deltas array");
    assert_eq!(deltas.len(), 1, "the one logged query must show up as a current delta: {review}");
    assert_eq!(review["changed_count"], 0, "nothing actually changed: {review}");
}
