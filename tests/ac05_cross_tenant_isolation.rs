//! AC5 — Given two tenants A and B, When A publishes `hello` and B sends
//! `tools/list` and `tools/call` on `A.hello`, Then B does not see it
//! listed and the call returns a JSON-RPC error, not a result.

use crate::common;
use common::{McpClient, TestServer, signup};
use serde_json::json;

#[tokio::test]
async fn tenant_b_cannot_see_or_call_tenant_a_tool() {
    let server = TestServer::start().await;

    let (tenant_a_ns, key_a) = signup(&server.base_url, "Tenant A").await;
    let client_a = McpClient::with_bearer(&server.base_url, &key_a);
    client_a
        .tools_call(
            "host.tool_publish",
            json!({"name": "hello", "kind": "echo", "spec": {"schema": {"type": "object"}}}),
        )
        .await
        .expect("A publishes hello");
    let qualified = format!("{tenant_a_ns}.hello");

    let (_tenant_b_ns, key_b) = signup(&server.base_url, "Tenant B").await;
    let client_b = McpClient::with_bearer(&server.base_url, &key_b);

    let tools = client_b.tools_list().await.expect("B tools/list");
    let names: Vec<&str> = tools["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert!(
        !names.contains(&qualified.as_str()),
        "B must not see A's tool: {names:?}"
    );

    let err = client_b
        .tools_call(&qualified, json!({}))
        .await
        .expect_err("B calling A's tool must return a JSON-RPC error, not a result");
    assert_eq!(err.error_code.as_deref(), Some("tool_not_found"));
}
