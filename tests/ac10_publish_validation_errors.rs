//! AC10 — Given a publish with an unregistered kind, an invalid name, or a
//! spec over 64 KiB, When `host.tool_publish` runs, Then each returns its
//! distinct error code and nothing is written.

use crate::common;
use common::{McpClient, TestServer, signup};
use serde_json::json;

#[tokio::test]
async fn each_validation_failure_has_its_own_code_and_writes_nothing() {
    let server = TestServer::start().await;
    let (tenant_ns, key) = signup(&server.base_url, "Validator").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let err = client
        .tools_call(
            "host.tool_publish",
            json!({"name": "hello", "kind": "not_a_real_kind", "spec": {}}),
        )
        .await
        .expect_err("unregistered kind must fail");
    assert_eq!(err.error_code.as_deref(), Some("unknown_kind"));
    assert!(
        err.message.contains("echo"),
        "unknown-kind error should name the registered kinds, got: {}",
        err.message
    );

    let err = client
        .tools_call(
            "host.tool_publish",
            json!({"name": "Not-Valid!", "kind": "echo", "spec": {"schema": {"type": "object"}}}),
        )
        .await
        .expect_err("invalid tool name must fail");
    assert_eq!(err.error_code.as_deref(), Some("invalid_tool_name"));

    let oversized_description = "x".repeat(70 * 1024);
    let err = client
        .tools_call(
            "host.tool_publish",
            json!({
                "name": "toobig",
                "kind": "echo",
                "spec": {"schema": {"type": "object", "description": oversized_description}},
            }),
        )
        .await
        .expect_err("oversized spec must fail");
    assert_eq!(err.error_code.as_deref(), Some("spec_too_large"));

    let tenant = server
        .state
        .db
        .find_tenant_by_namespace(tenant_ns)
        .await
        .unwrap()
        .expect("tenant exists");
    let tools = server.state.db.list_tools(tenant.id).await.unwrap();
    assert!(
        tools.is_empty(),
        "no failed publish should have written a tool row: {tools:?}"
    );
}
