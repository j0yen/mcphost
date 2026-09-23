//! AC2 (PRD-mcphost-signup-kill-switch-and-source) — Given `signup(name)`
//! with no source, When it succeeds, Then `signup_source` is NULL and the
//! response omits or nulls `source`.

use crate::common;
use common::{TestServer, extract_structured, signup};

#[tokio::test]
async fn signup_without_source_is_null_and_omitted() {
    let server = TestServer::start().await;
    let (ns, _key) = signup(&server.base_url, "No Source Agent").await;

    let client = common::McpClient::new(&server.base_url);
    let result = client
        .tools_call("signup", serde_json::json!({"name": "Second No Source Agent"}))
        .await
        .expect("signup without source");
    let structured = extract_structured(&result);
    assert!(
        structured.get("source").is_none() || structured["source"].is_null(),
        "response must omit or null source, got {structured:?}"
    );

    let tenant = server
        .state
        .db
        .find_tenant_by_namespace(ns)
        .await
        .expect("query")
        .expect("tenant exists");
    assert_eq!(tenant.signup_source, None);
}
