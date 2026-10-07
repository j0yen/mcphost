//! PRD-mcphost-kind-ask-routing
//! AC7 (P2) -- Given `host.tool_publish {kind: http, upstream:
//! ".../database/query/..."}`, When published, Then the response carries
//! `hint: database-in-a-minute`.

use crate::common;
use common::{McpClient, TestServer, extract_structured, http_kind_registry, signup};
use serde_json::json;

async fn publish(client: &McpClient, name: &str, url: &str) -> serde_json::Value {
    let result = client
        .tools_call(
            "host.tool_publish",
            json!({
                "name": name,
                "kind": "http",
                "spec": {"upstream": {"url": url, "method": "GET"}},
            }),
        )
        .await
        .expect("publish ok");
    extract_structured(&result)
}

#[tokio::test]
async fn http_tool_over_a_database_query_path_carries_the_recipe_hint() {
    let server = TestServer::start_with_kinds(http_kind_registry()).await;
    let (_ns, key) = signup(&server.base_url, "Kindroute Ask AC7 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let hinted = publish(&client, "dbq", "http://127.0.0.1:9/database/query/orders").await;
    assert_eq!(hinted["hint"], json!("database-in-a-minute"), "response: {hinted}");

    let plain = publish(&client, "orders", "http://127.0.0.1:9/orders").await;
    assert!(plain.get("hint").is_none(), "no hint off the database path: {plain}");
}
