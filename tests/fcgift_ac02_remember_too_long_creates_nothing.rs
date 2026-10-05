//! PRD-mcphost-first-call-gift
//! AC2 — Given `remember` of 4097 bytes, When `signup` is called, Then
//! `remember_too_long` is returned and no tenant and no state row is
//! created.

use crate::common;
use common::{McpClient, TestServer};
use serde_json::json;

#[tokio::test]
async fn oversized_remember_refuses_and_creates_no_tenant() {
    let server = TestServer::start().await;
    let before = server.state.db.list_tenants().await.expect("list tenants before").len();

    let client = McpClient::new(&server.base_url);
    let remember = "x".repeat(4097);
    let err = client
        .tools_call("signup", json!({"name": "AC2 Tenant", "remember": remember}))
        .await
        .expect_err("an over-long remember must refuse signup");

    assert_eq!(err.error_code.as_deref(), Some("remember_too_long"), "{err:?}");

    let after = server.state.db.list_tenants().await.expect("list tenants after").len();
    assert_eq!(before, after, "an over-long remember must create no tenant");
    assert_eq!(after, 0, "this fresh server should have no tenants at all");
}
