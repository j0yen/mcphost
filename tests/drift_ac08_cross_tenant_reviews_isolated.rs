//! PRD-mcphost-drift-review
//! AC8 -- Given two tenants, When tenant A reads reviews, Then none of
//! tenant B's appear.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn tenant_a_never_sees_tenant_b_reviews() {
    let server = TestServer::start().await;
    let (_ns_a, key_a) = signup(&server.base_url, "Drift AC8 Tenant A").await;
    let (_ns_b, key_b) = signup(&server.base_url, "Drift AC8 Tenant B").await;
    let client_a = McpClient::with_bearer(&server.base_url, &key_a);
    let client_b = McpClient::with_bearer(&server.base_url, &key_b);

    for (client, table) in [(&client_a, "a_table"), (&client_b, "b_table")] {
        client
            .tools_call("host.table.create", json!({"name": table, "columns": {"n": "integer"}}))
            .await
            .unwrap_or_else(|e| panic!("create {table} failed: {} {}", e.code, e.message));
        client
            .tools_call("host.table.model_set", json!({"table": table, "key": "description", "value": "note"}))
            .await
            .unwrap_or_else(|e| panic!("model_set {table} failed: {} {}", e.code, e.message));
    }
    mcphost::tables_model::tick_once(&server.state).await.expect("tick_once");

    let reviews_a = extract_structured(
        &client_a.tools_call("host.drift.reviews", json!({})).await.expect("reviews a"),
    );
    let targets_a: Vec<_> = reviews_a["reviews"]
        .as_array()
        .expect("reviews array")
        .iter()
        .map(|r| r["target"].as_str().unwrap_or_default().to_string())
        .collect();
    assert!(targets_a.contains(&"a_table".to_string()), "tenant A must see its own review: {targets_a:?}");
    assert!(
        !targets_a.contains(&"b_table".to_string()),
        "tenant A must never see tenant B's review: {targets_a:?}"
    );

    let reviews_b = extract_structured(
        &client_b.tools_call("host.drift.reviews", json!({})).await.expect("reviews b"),
    );
    let targets_b: Vec<_> = reviews_b["reviews"]
        .as_array()
        .expect("reviews array")
        .iter()
        .map(|r| r["target"].as_str().unwrap_or_default().to_string())
        .collect();
    assert!(targets_b.contains(&"b_table".to_string()), "tenant B must see its own review: {targets_b:?}");
    assert!(
        !targets_b.contains(&"a_table".to_string()),
        "tenant B must never see tenant A's review: {targets_b:?}"
    );
}
