//! PRD-mcphost-tenant-delete
//! AC2 — Given a tenant key, When it calls `admin.tenant_delete` or
//! `admin.tenant_delete_by_prefix`, Then the call returns `forbidden` and
//! nothing changes.

mod common;
use common::{McpClient, TestServer, signup};
use serde_json::json;

#[tokio::test]
async fn tenant_key_cannot_reach_either_delete_tool() {
    let server = TestServer::start().await;
    let (tenant_ns, key) = signup(&server.base_url, "Regular Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let err = client
        .tools_call("admin.tenant_delete", json!({"tenant": tenant_ns}))
        .await
        .expect_err("a tenant key must never reach admin.tenant_delete");
    assert_eq!(err.error_code.as_deref(), Some("forbidden"));

    let err = client
        .tools_call(
            "admin.tenant_delete_by_prefix",
            json!({"prefix": "regu", "dry_run": false}),
        )
        .await
        .expect_err("a tenant key must never reach admin.tenant_delete_by_prefix");
    assert_eq!(err.error_code.as_deref(), Some("forbidden"));

    // Nothing changed: the tenant is still there.
    let tenant = server
        .state
        .db
        .find_tenant_by_namespace(tenant_ns)
        .await
        .unwrap();
    assert!(tenant.is_some(), "the tenant must survive both refused calls");
}
