//! AC1 (PRD-mcphost-signup-kill-switch-and-source) — Given
//! `signup(name, source: "hn")`, When it succeeds, Then the response
//! echoes `source: "hn"` and the tenant row has `signup_source = 'hn'`.

use crate::common;
use common::{McpClient, TestServer, extract_structured};
use serde_json::json;

#[tokio::test]
async fn signup_with_source_echoes_and_stores_it() {
    let server = TestServer::start().await;
    let client = McpClient::new(&server.base_url);

    let result = client
        .tools_call("signup", json!({"name": "HN Agent", "source": "hn"}))
        .await
        .expect("signup with source");
    let structured = extract_structured(&result);
    assert_eq!(structured["source"], json!("hn"), "{structured:?}");

    let ns = structured["tenant"].as_str().expect("tenant field").to_string();
    let tenant = server
        .state
        .db
        .find_tenant_by_namespace(ns)
        .await
        .expect("query")
        .expect("tenant exists");
    assert_eq!(tenant.signup_source.as_deref(), Some("hn"));
}
