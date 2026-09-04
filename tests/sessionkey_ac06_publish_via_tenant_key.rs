//! PRD-mcphost-session-key
//! AC6 — Given a valid `tenant_key`, When a client with no `Authorization`
//! header calls `host.tool_publish` with a valid `echo` spec, Then the tool
//! is published under that tenant's namespace and `host.tool_list` with the
//! same key returns it.

mod common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn publish_and_list_work_over_tenant_key_alone() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "No-Header Publisher").await;
    let client = McpClient::new(&server.base_url); // no bearer at all

    let publish = client
        .tools_call(
            "host.tool_publish",
            json!({
                "name": "hello",
                "kind": "echo",
                "spec": {"schema": {"type": "object"}},
                "tenant_key": key,
            }),
        )
        .await
        .expect("publish via tenant_key");
    let published = extract_structured(&publish);
    assert_eq!(published["name"].as_str(), Some(format!("{ns}.hello").as_str()));

    let list = client
        .tools_call("host.tool_list", json!({"tenant_key": key}))
        .await
        .expect("tool_list via tenant_key");
    let tools = extract_structured(&list)["tools"]
        .as_array()
        .expect("tools array")
        .clone();
    assert!(
        tools
            .iter()
            .any(|t| t["name"] == json!(format!("{ns}.hello"))),
        "the tool published via tenant_key must be listed via tenant_key: {tools:?}"
    );
}
