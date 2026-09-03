//! AC6 — Given a tenant with a published tool, When it calls
//! `host.tool_remove("hello")`, Then the next `tools/list` omits it and a
//! call to it returns error `tool_not_found`.

mod common;
use common::{McpClient, TestServer, signup};
use serde_json::json;

#[tokio::test]
async fn remove_then_list_omits_and_call_errors() {
    let server = TestServer::start().await;
    let (tenant_ns, key) = signup(&server.base_url, "Remover").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "hello", "kind": "echo", "spec": {"schema": {"type": "object"}}}),
        )
        .await
        .expect("publish");

    client
        .tools_call("host.tool_remove", json!({"name": "hello"}))
        .await
        .expect("remove");

    let tools = client.tools_list().await.expect("tools/list");
    let names: Vec<&str> = tools["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    let qualified = format!("{tenant_ns}.hello");
    assert!(
        !names.contains(&qualified.as_str()),
        "removed tool must not be listed: {names:?}"
    );

    let err = client
        .tools_call(&qualified, json!({}))
        .await
        .expect_err("calling a removed tool must error");
    assert_eq!(err.error_code.as_deref(), Some("tool_not_found"));
}
