//! PRD-mcphost-tenant-key-missing-is-invalid-params
//! AC5 (P0) — Given an argument-only call with a `tenant_key` that matches
//! no tenant, When the server responds, Then the numeric code is still
//! `-32600` and `data.error_code` is `tenant_key_invalid` (only one variant
//! moved).

use crate::common;
use common::{McpClient, TestServer};
use serde_json::json;

#[tokio::test]
async fn unrecognized_tenant_key_stays_invalid_request() {
    let server = TestServer::start().await;
    let client = McpClient::new(&server.base_url);

    let err = client
        .tools_call(
            "host.tool_publish",
            json!({
                "name": "my_tool",
                "kind": "echo",
                "spec": {},
                "tenant_key": "this-key-matches-no-tenant",
            }),
        )
        .await
        .expect_err("an unrecognized tenant_key must be refused");

    assert_eq!(err.code, -32600, "must stay INVALID_REQUEST: {err:?}");
    assert_eq!(err.error_code.as_deref(), Some("tenant_key_invalid"));
}
