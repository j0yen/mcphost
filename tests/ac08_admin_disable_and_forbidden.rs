//! AC8 — Given the admin key, When `admin.tenant_disable(t)` runs, Then
//! the next request with t's key returns error `tenant_disabled`; Given a
//! tenant key sent to `admin.tenants`, Then the call is refused with
//! `forbidden`.

use crate::common;
use common::{ADMIN_KEY, McpClient, TestServer, signup};
use serde_json::json;

#[tokio::test]
async fn disabling_a_tenant_locks_out_its_key() {
    let server = TestServer::start().await;
    let (tenant_ns, key) = signup(&server.base_url, "To Be Disabled").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    let admin = McpClient::with_bearer(&server.base_url, ADMIN_KEY);

    admin
        .tools_call("admin.tenant_disable", json!({"tenant": tenant_ns}))
        .await
        .expect("admin.tenant_disable");

    let err = client
        .tools_call("host.whoami", json!({}))
        .await
        .expect_err("a disabled tenant's key must be rejected");
    assert_eq!(err.error_code.as_deref(), Some("tenant_disabled"));

    let err = client
        .tools_list()
        .await
        .expect_err("tools/list with a disabled tenant's key must also be rejected");
    assert_eq!(err.error_code.as_deref(), Some("tenant_disabled"));
}

#[tokio::test]
async fn tenant_key_calling_admin_tenants_is_forbidden() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Regular Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let err = client
        .tools_call("admin.tenants", json!({}))
        .await
        .expect_err("a tenant key must never reach admin.tenants");
    assert_eq!(err.error_code.as_deref(), Some("forbidden"));
}
