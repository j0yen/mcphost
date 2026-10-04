//! PRD-mcphost-implicit-signup
//! AC6 (P0) — Given a request with an unknown `tenant_key`, When dispatched,
//! Then `tenant_key_invalid` is returned and no tenant is created.

use crate::common;
use common::{McpClient, TestServer};
use serde_json::json;

#[tokio::test]
async fn unknown_tenant_key_is_invalid_not_an_implicit_signup() {
    let server = TestServer::start().await;
    let client = McpClient::new(&server.base_url);

    let err = client
        .tools_call("host.whoami", json!({"tenant_key": "not-a-real-key"}))
        .await
        .expect_err("an unrecognized tenant_key must be refused, not silently signed up");
    assert_eq!(err.error_code.as_deref(), Some("tenant_key_invalid"), "{err:?}");

    let tenants = server.state.db.list_tenants().await.expect("list tenants");
    assert_eq!(tenants.len(), 0, "an invalid tenant_key must never create a tenant");
}
