//! PRD-mcphost-tenant-key-missing-is-invalid-params
//! AC7 (P1) — Given the `example` string from AC 6, When it is passed as
//! `tenant_key` on a fresh argument-only call, Then the server returns
//! `tenant_key_invalid`, not success (the example is a placeholder, never a
//! live key).

use crate::common;
use common::{McpClient, TestServer};
use serde_json::json;

#[tokio::test]
async fn the_example_tenant_key_never_resolves_to_a_tenant() {
    let server = TestServer::start().await;
    let client = McpClient::new(&server.base_url);

    let refusal = client
        .tools_call(
            "host.tool_publish",
            json!({"name": "my_tool", "kind": "echo", "spec": {}}),
        )
        .await
        .expect_err("a call with no tenant_key at all must be refused");
    let example = refusal
        .data
        .get("example")
        .and_then(|v| v.as_str())
        .expect("data.example must be a string")
        .to_string();

    let err = client
        .tools_call(
            "host.tool_publish",
            json!({"name": "my_tool", "kind": "echo", "spec": {}, "tenant_key": example}),
        )
        .await
        .expect_err("the example must never be a live key that resolves to a tenant");

    assert_eq!(
        err.error_code.as_deref(),
        Some("tenant_key_invalid"),
        "the example tenant_key must read as unrecognized, not missing or successful: {err:?}"
    );
}
