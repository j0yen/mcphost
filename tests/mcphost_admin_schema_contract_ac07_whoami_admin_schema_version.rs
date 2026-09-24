//! PRD-mcphost-admin-schema-contract
//! AC7 (P2) — Given an admin-scoped tenant, When `host.whoami` is called,
//! Then `admin_schema_version` is present.

use crate::common;
use common::{ADMIN_KEY, McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn admin_key_calling_host_whoami_sees_admin_schema_version() {
    let server = TestServer::start().await;
    let admin = McpClient::with_bearer(&server.base_url, ADMIN_KEY);

    let result = admin
        .tools_call("host.whoami", json!({}))
        .await
        .expect("admin key must be able to call host.whoami");
    let whoami = extract_structured(&result);

    assert_eq!(whoami["admin_schema_version"], json!(1), "{whoami:?}");
}

#[tokio::test]
async fn a_tenant_key_calling_host_whoami_never_sees_admin_schema_version() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Not Admin").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let result = client
        .tools_call("host.whoami", json!({}))
        .await
        .expect("host.whoami");
    let whoami = extract_structured(&result);

    assert!(
        whoami.get("admin_schema_version").is_none(),
        "a tenant's own host.whoami must never carry admin_schema_version: {whoami:?}"
    );
}
