//! PRD-mcphost-tool-test AC5 — Given a spec that fails validation (malformed
//! schema, oversized source, unknown kind), When tested, Then the error is
//! structured, names the offending field, and uses the same error taxonomy
//! as `host.tool_publish`.

mod common;
use common::{McpClient, TestServer, signup};
use serde_json::json;

#[tokio::test]
async fn malformed_echo_schema_is_invalid_spec() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Bad Schema Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    // No `schema` field at all -- the same rejection host.tool_publish gives
    // an echo spec missing it.
    let err = client
        .tools_call(
            "host.spec_test",
            json!({"kind": "echo", "spec": {}, "invocations": [{}]}),
        )
        .await
        .expect_err("missing schema must be rejected");
    assert_eq!(err.error_code.as_deref(), Some("invalid_spec"));
}

#[tokio::test]
async fn unknown_kind_is_rejected() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Unknown Kind Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let err = client
        .tools_call(
            "host.spec_test",
            json!({"kind": "wasm", "spec": {}, "invocations": [{}]}),
        )
        .await
        .expect_err("an unregistered kind must be rejected");
    assert_eq!(err.error_code.as_deref(), Some("unknown_kind"));
}

#[tokio::test]
async fn same_taxonomy_as_tool_publish_for_the_same_bad_spec() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Taxonomy Parity Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let bad_spec = json!({});
    let publish_err = client
        .tools_call(
            "host.tool_publish",
            json!({"name": "bad_one", "kind": "echo", "spec": bad_spec}),
        )
        .await
        .expect_err("publish of a schema-less echo spec must fail");
    let test_err = client
        .tools_call(
            "host.spec_test",
            json!({"kind": "echo", "spec": bad_spec, "invocations": [{}]}),
        )
        .await
        .expect_err("spec_test of the same spec must fail the same way");
    assert_eq!(publish_err.error_code, test_err.error_code);
}
