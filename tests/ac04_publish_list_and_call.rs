//! AC4 — Given a tenant, When it calls
//! `host.tool_publish("hello", "echo", {"schema": {...}})` and then
//! `tools/list`, Then `t_xxxxxxxx.hello` is listed on that same
//! connection's next request, and `tools/call` on it returns the
//! arguments passed.

mod common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn publish_then_list_then_call_round_trips() {
    let server = TestServer::start().await;
    let (tenant_ns, key) = signup(&server.base_url, "Publisher").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let publish = client
        .tools_call(
            "host.tool_publish",
            json!({
                "name": "hello",
                "kind": "echo",
                "spec": {"schema": {"type": "object", "properties": {"msg": {"type": "string"}}, "required": ["msg"]}},
            }),
        )
        .await
        .expect("publish should succeed");
    let qualified = extract_structured(&publish)["name"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(qualified, format!("{tenant_ns}.hello"));

    let tools = client.tools_list().await.expect("tools/list");
    let names: Vec<&str> = tools["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert!(
        names.contains(&qualified.as_str()),
        "published tool must appear in tools/list: {names:?}"
    );

    let call = client
        .tools_call(&qualified, json!({"msg": "hi there"}))
        .await
        .expect("call should succeed");
    let result = extract_structured(&call);
    assert_eq!(
        result,
        json!({"msg": "hi there"}),
        "echo must return its arguments"
    );
}
