//! PRD-mcphost-session-key
//! AC16 — Given a single HTTP connection with fixed headers and no
//! `Authorization`, When a client performs `initialize`, `tools/list`,
//! `signup`, `host.tool_publish` and `host.tool_call` in sequence using
//! only the key returned by `signup`, Then every step succeeds and the
//! final call returns the published tool's output. This is the end-to-end
//! proof of the PRD's whole premise: one connection, static headers,
//! signup to first call, no reconnect.

mod common;
use common::{McpClient, TestServer, extract_structured};
use serde_json::json;

#[tokio::test]
async fn signup_publish_and_call_complete_on_one_connection_with_no_header() {
    let server = TestServer::start().await;
    let client = McpClient::new(&server.base_url); // never carries Authorization

    // 1. initialize
    let init = client.initialize().await;
    assert!(init.get("error").is_none(), "initialize failed: {init:?}");

    // 2. tools/list -- the control plane must already be visible.
    let tools = client.tools_list().await.expect("tools/list");
    let names: Vec<&str> = tools["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"signup"));
    assert!(names.contains(&"host.tool_publish"));
    assert!(names.contains(&"host.tool_call"));

    // 3. signup -- the only credential this session will ever have.
    let signup_result = extract_structured(
        &client
            .tools_call("signup", json!({"name": "One-Connection Agent"}))
            .await
            .expect("signup"),
    );
    let key = signup_result["key"].as_str().expect("key").to_string();

    // 4. host.tool_publish, carrying the key as tenant_key -- no header ever set.
    let publish = client
        .tools_call(
            "host.tool_publish",
            json!({
                "name": "hello",
                "kind": "echo",
                "spec": {"schema": {"type": "object", "properties": {"msg": {"type": "string"}}, "required": ["msg"]}},
                "tenant_key": key,
            }),
        )
        .await
        .expect("host.tool_publish over tenant_key");
    assert!(extract_structured(&publish)["name"].as_str().unwrap().ends_with(".hello"));

    // 5. host.tool_call -- the first real, metered call, reached without a
    // reconnect and without ever seeing the namespaced tool name.
    let call = client
        .tools_call(
            "host.tool_call",
            json!({"name": "hello", "args": {"msg": "hi there"}, "tenant_key": key}),
        )
        .await
        .expect("host.tool_call over tenant_key");
    let result = extract_structured(&call);
    assert_eq!(
        result,
        json!({"msg": "hi there"}),
        "the published echo tool's output must come back: {result}"
    );
}
