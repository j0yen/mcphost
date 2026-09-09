//! PRD-mcphost-auth-error-names-argument
//! AC1 (P0) — Given a `host.tool_publish` call with no `tenant_key`, When
//! it runs, Then the error has `error_code` `tenant_key_missing` and its
//! message names `tenant_key` and `signup` and does not contain
//! `Authorization`.

mod common;
use common::{McpClient, TestServer};
use serde_json::json;

#[tokio::test]
async fn missing_tenant_key_names_the_argument_not_the_header() {
    let server = TestServer::start().await;
    // No Authorization header, and no tenant_key argument either.
    let client = McpClient::new(&server.base_url);

    let err = client
        .tools_call(
            "host.tool_publish",
            json!({"name": "my_tool", "kind": "echo", "spec": {}}),
        )
        .await
        .expect_err("a call with no tenant_key at all must be refused");

    assert_eq!(err.error_code.as_deref(), Some("tenant_key_missing"));
    assert!(
        err.message.contains("tenant_key"),
        "message must name the tenant_key argument: {}",
        err.message
    );
    assert!(
        err.message.contains("signup"),
        "message must point at signup as the source of the key: {}",
        err.message
    );
    assert!(
        !err.message.contains("Authorization"),
        "message must not name the header the caller cannot send: {}",
        err.message
    );
    // Requirement 1: data.docs still points at host.quickstart, same as
    // every other rejection (AppError::into_error_data sets this
    // unconditionally).
    assert_eq!(err.data.get("docs").and_then(|v| v.as_str()), Some("host.quickstart"));
}

#[tokio::test]
async fn a_non_string_tenant_key_counts_as_missing_too() {
    let server = TestServer::start().await;
    let client = McpClient::new(&server.base_url);

    let err = client
        .tools_call("host.tool_list", json!({"tenant_key": 12345}))
        .await
        .expect_err("a non-string tenant_key must be treated as absent");

    assert_eq!(err.error_code.as_deref(), Some("tenant_key_missing"));
}
