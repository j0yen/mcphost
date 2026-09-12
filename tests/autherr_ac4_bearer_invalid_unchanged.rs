//! PRD-mcphost-auth-error-names-argument
//! AC4 (P0) — Given an admin tool call with an invalid bearer header, When
//! it runs, Then the message is `missing or invalid Authorization: Bearer
//! key` and `error_code` is `bearer_invalid`.

use crate::common;
use common::{McpClient, TestServer};
use serde_json::json;

#[tokio::test]
async fn invalid_bearer_keeps_its_message_and_gains_bearer_invalid_code() {
    let server = TestServer::start().await;
    let client = McpClient::with_bearer(&server.base_url, "not-a-real-key-at-all");

    let err = client
        .tools_call("admin.tenants", json!({}))
        .await
        .expect_err("an invalid bearer must be refused");

    assert_eq!(err.error_code.as_deref(), Some("bearer_invalid"));
    assert_eq!(err.message, "missing or invalid Authorization: Bearer key");
}

#[tokio::test]
async fn no_header_at_all_on_an_admin_tool_still_falls_through_to_the_argument_codes() {
    // Auth resolution (resolve_auth / resolve_tenant_key_auth) runs before
    // call_tool dispatches on the tool name, so a fully anonymous caller --
    // no Authorization header, no tenant_key argument -- gets the same
    // tenant_key_missing code an anonymous host.* caller would (AC1),
    // even though admin.tenants itself would go on to answer `forbidden`
    // for any *authenticated* non-admin caller. This is the one case AC4's
    // "admin ... paths keep their current messages" doesn't cover: no
    // header was ever sent, so there is no bearer-shaped failure to keep.
    let server = TestServer::start().await;
    let client = McpClient::new(&server.base_url);

    let err = client
        .tools_call("admin.tenants", json!({}))
        .await
        .expect_err("an anonymous caller of an admin tool must be refused");

    assert_eq!(err.error_code.as_deref(), Some("tenant_key_missing"));
}
