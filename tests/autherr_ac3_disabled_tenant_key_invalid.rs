//! PRD-mcphost-auth-error-names-argument
//! AC3 (P0) — Given a disabled tenant's key, When a `host.*` tool is
//! called, Then the error is `tenant_key_invalid`.
//!
//! Scoped to the `tenant_key` *argument* path (this PRD's subject);
//! `tests/ac08_admin_disable_and_forbidden.rs` pins the unchanged
//! `tenant_disabled` code for the header path.

use crate::common;
use common::{ADMIN_KEY, McpClient, TestServer, signup};
use serde_json::json;

#[tokio::test]
async fn disabled_tenants_key_reads_as_tenant_key_invalid_via_the_argument() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "About To Be Disabled Again").await;
    let admin = McpClient::with_bearer(&server.base_url, ADMIN_KEY);
    admin
        .tools_call("admin.tenant_disable", json!({"tenant": ns}))
        .await
        .expect("admin.tenant_disable");

    let anon_client = McpClient::new(&server.base_url);
    let err = anon_client
        .tools_call("host.whoami", json!({"tenant_key": key}))
        .await
        .expect_err("a disabled tenant's key sent as tenant_key must be refused");

    assert_eq!(err.error_code.as_deref(), Some("tenant_key_invalid"));
    assert!(
        !err.message.contains(&key),
        "message must never echo the disabled tenant's key: {}",
        err.message
    );
}

#[tokio::test]
async fn header_path_disabled_tenant_still_reads_tenant_disabled() {
    // Sanity check that this PRD did not touch the header path's own,
    // separately-pinned behavior (requirement 3's "admin and
    // header-authenticated paths keep their current messages").
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "Disabled Via Header Path").await;
    let admin = McpClient::with_bearer(&server.base_url, ADMIN_KEY);
    admin
        .tools_call("admin.tenant_disable", json!({"tenant": ns}))
        .await
        .expect("admin.tenant_disable");

    let client = McpClient::with_bearer(&server.base_url, &key);
    let err = client
        .tools_call("host.whoami", json!({}))
        .await
        .expect_err("a disabled tenant's bearer key must be refused");
    assert_eq!(err.error_code.as_deref(), Some("tenant_disabled"));
}
