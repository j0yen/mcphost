//! AC2 — Given the same tool private (never shared), When B calls it, Then
//! `tool_not_found`. This is the pre-existing cross-tenant-isolation
//! behavior (AC5 of PRD-mcphost-composition's predecessor, `ac05_cross_tenant_isolation.rs`);
//! this test pins that it still holds now that `<ns>.<name>` for another
//! tenant is a real resolution path instead of an unconditional refusal.

mod common;
use common::{McpClient, TestServer, signup};
use serde_json::json;

#[tokio::test]
async fn private_tool_is_not_found_for_another_tenant() {
    let server = TestServer::start().await;

    let (ns_a, key_a) = signup(&server.base_url, "Tenant A").await;
    let client_a = McpClient::with_bearer(&server.base_url, &key_a);
    client_a
        .tools_call(
            "host.tool_publish",
            json!({"name": "geo", "kind": "echo", "spec": {"schema": {"type": "object"}}}),
        )
        .await
        .expect("A publishes geo, never shares it");

    let (_ns_b, key_b) = signup(&server.base_url, "Tenant B").await;
    let client_b = McpClient::with_bearer(&server.base_url, &key_b);
    let qualified = format!("{ns_a}.geo");
    let err = client_b
        .tools_call(&qualified, json!({}))
        .await
        .expect_err("B calling A's private tool must error");
    assert_eq!(err.error_code.as_deref(), Some("tool_not_found"));
}
