//! PRD-mcphost-auth-error-names-argument
//! AC1 (P0) — Given a call with no `tenant_key`, When it runs, Then the
//! error has `error_code` `tenant_key_missing` and its message names
//! `tenant_key` and `signup` and does not contain `Authorization`.
//!
//! PRD-mcphost-implicit-signup: a bare `host.*`/`billing.*` call on `/mcp`
//! with no `tenant_key` at all no longer returns `tenant_key_missing` --
//! it implicitly signs up instead (see `tests/implsign_ac01_*.rs`). This
//! AC's own point (the error's naming/message shape) is still real for
//! every OTHER anonymous call `admin.*` never implicit-signs-up (Non-goal),
//! so `admin.tenants` is used here as the conduit instead of
//! `host.tool_publish`.

use crate::common;
use common::{McpClient, TestServer};
use serde_json::json;

#[tokio::test]
async fn missing_tenant_key_names_the_argument_not_the_header() {
    let server = TestServer::start().await;
    // No Authorization header, and no tenant_key argument either.
    let client = McpClient::new(&server.base_url);

    let err = client
        .tools_call("admin.tenants", json!({}))
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
        .tools_call("admin.tenants", json!({"tenant_key": 12345}))
        .await
        .expect_err("a non-string tenant_key must be treated as absent");

    assert_eq!(err.error_code.as_deref(), Some("tenant_key_missing"));
}
