//! PRD-mcphost-drift-review
//! AC3 -- Given the same change applied twice with identical content,
//! When both are processed, Then one review item exists.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn identical_content_applied_twice_yields_one_review() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "Drift AC3 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    let tenant = server.state.db.find_tenant_by_namespace(ns).await.expect("find tenant").expect("tenant exists");

    client
        .tools_call("host.table.create", json!({"name": "expenses", "columns": {"amount": "real"}}))
        .await
        .expect("create expenses");

    for _ in 0..2 {
        client
            .tools_call(
                "host.table.model_set",
                json!({"table": "expenses", "key": "description", "value": "identical note text"}),
            )
            .await
            .expect("model_set");
    }

    // Two distinct versions were recorded (the dedupe is at review-emission
    // time, not at versioning time).
    let latest = server
        .state
        .db
        .latest_context_version(tenant.id, "table_note".to_string(), "expenses".to_string())
        .await
        .expect("latest_context_version");
    assert_eq!(latest, Some(2), "two context_versions rows must exist: {latest:?}");

    mcphost::tables_model::tick_once(&server.state).await.expect("tick_once");

    let reviews = extract_structured(
        &client.tools_call("host.drift.reviews", json!({})).await.expect("reviews"),
    );
    let matching: Vec<_> = reviews["reviews"]
        .as_array()
        .expect("reviews array")
        .iter()
        .filter(|r| r["target"] == "expenses" && r["kind"] == "table_note")
        .collect();
    assert_eq!(matching.len(), 1, "exactly one review item must exist for the identical-content change: {reviews}");
}
