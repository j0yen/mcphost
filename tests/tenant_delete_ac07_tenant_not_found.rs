//! PRD-mcphost-tenant-delete
//! AC7 — Given an unknown tenant id, When `admin.tenant_delete` runs,
//! Then it returns `tenant_not_found`.

use crate::common;
use common::{ADMIN_KEY, McpClient, TestServer};
use serde_json::json;

#[tokio::test]
async fn deleting_an_unknown_tenant_returns_tenant_not_found() {
    let server = TestServer::start().await;
    let admin = McpClient::with_bearer(&server.base_url, ADMIN_KEY);

    let err = admin
        .tools_call(
            "admin.tenant_delete",
            json!({"tenant": "no-such-tenant-namespace"}),
        )
        .await
        .expect_err("an unknown tenant must not succeed");
    assert_eq!(err.error_code.as_deref(), Some("tenant_not_found"));
}
