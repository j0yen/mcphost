//! PRD-mcphost-drift-review
//! AC7 -- Given `host.drift.resolve(item_id, "cosmetic")`, When
//! `host.drift.reviews(open_only: true)` is called, Then the item is
//! absent and `host.drift.review(item_id)` shows the reason.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn resolve_removes_from_open_only_and_review_shows_reason() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Drift AC7 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call("host.table.create", json!({"name": "widgets", "columns": {"n": "integer"}}))
        .await
        .expect("create widgets");
    client
        .tools_call(
            "host.table.model_set",
            json!({"table": "widgets", "key": "description", "value": "widget count"}),
        )
        .await
        .expect("model_set");
    mcphost::tables_model::tick_once(&server.state).await.expect("tick_once");

    let open_before = extract_structured(
        &client.tools_call("host.drift.reviews", json!({"open_only": true})).await.expect("reviews"),
    );
    let summary = open_before["reviews"]
        .as_array()
        .expect("reviews array")
        .iter()
        .find(|r| r["target"] == "widgets")
        .unwrap_or_else(|| panic!("no open review for widgets: {open_before}"))
        .clone();
    let item_id = summary["item_id"].as_str().expect("item_id").to_string();
    assert_eq!(summary["reason"], json!(null), "a freshly produced review must be open: {summary}");

    let resolved = extract_structured(
        &client
            .tools_call("host.drift.resolve", json!({"item_id": item_id, "reason": "cosmetic"}))
            .await
            .expect("resolve"),
    );
    assert_eq!(resolved["resolved"], true, "resolve: {resolved}");

    let open_after = extract_structured(
        &client.tools_call("host.drift.reviews", json!({"open_only": true})).await.expect("reviews"),
    );
    assert!(
        !open_after["reviews"].as_array().expect("reviews array").iter().any(|r| r["item_id"] == json!(item_id)),
        "the resolved item must be absent from open_only: true: {open_after}"
    );

    let review = extract_structured(
        &client.tools_call("host.drift.review", json!({"item_id": item_id})).await.expect("review"),
    );
    assert_eq!(review["reason"], json!("cosmetic"), "review: {review}");
}
