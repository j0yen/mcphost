//! PRD-mcphost-tool-test:
//! AC9 (P1) — Given an unauthenticated connection, When it lists tools,
//! Then `host.spec_test` is absent, and calling it is refused like any
//! other `host.*` tool.
//! AC10 (P1) — Given an authenticated tenant, When it lists tools, Then
//! `host.spec_test` appears with an input schema documenting `kind`,
//! `spec`, and `invocations`.

use crate::common;
use common::{McpClient, TestServer, signup};
use serde_json::json;

#[tokio::test]
async fn absent_from_the_anonymous_list_and_refused_when_called() {
    let server = TestServer::start().await;
    let client = McpClient::new(&server.base_url);

    let tools = client.tools_list().await.expect("tools/list");
    let names: Vec<&str> = tools["tools"]
        .as_array()
        .expect("tools array")
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert!(
        !names.contains(&"host.spec_test"),
        "host.spec_test must not be listed for an anonymous caller: {names:?}"
    );

    let err = client
        .tools_call(
            "host.spec_test",
            json!({"kind": "echo", "spec": {}, "invocations": []}),
        )
        .await
        .expect_err("an anonymous caller must be refused");
    // PRD-mcphost-auth-error-names-argument requirement 1 / AC1: no
    // Authorization header and no tenant_key argument at all is
    // tenant_key_missing, not the old header-shaped "unauthorized".
    assert_eq!(err.error_code.as_deref(), Some("tenant_key_missing"));
}

#[tokio::test]
async fn present_for_an_authenticated_tenant_with_the_documented_schema() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Listing Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let tools = client.tools_list().await.expect("tools/list");
    let spec_test = tools["tools"]
        .as_array()
        .expect("tools array")
        .iter()
        .find(|t| t["name"] == json!("host.spec_test"))
        .expect("host.spec_test must be listed for an authenticated tenant")
        .clone();
    let props = spec_test["inputSchema"]["properties"]
        .as_object()
        .expect("properties object");
    assert!(props.contains_key("kind"));
    assert!(props.contains_key("spec"));
    assert!(props.contains_key("invocations"));
}
