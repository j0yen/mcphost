//! PRD-mcphost-shared-tool-call-path
//! AC4 — Given B calls `host.tool_call {name: "<O>.lookup", async: true}`,
//! When B calls `host.runs.wait {id}`, Then the result is returned; When O
//! calls `host.runs.get {id}`, Then `not_found`.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn async_qualified_call_is_scoped_to_the_caller_not_the_owner() {
    let server = TestServer::start().await;

    let (ns_o, key_o) = signup(&server.base_url, "Owner").await;
    let client_o = McpClient::with_bearer(&server.base_url, &key_o);
    client_o
        .tools_call(
            "host.tool_publish",
            json!({"name": "lookup", "kind": "echo", "spec": {"schema": {"type": "object"}}}),
        )
        .await
        .expect("O publishes lookup");
    client_o
        .tools_call(
            "host.tool_share",
            json!({"name": "lookup", "visibility": "public", "description": "lookup"}),
        )
        .await
        .expect("O shares lookup publicly");

    let (_ns_b, key_b) = signup(&server.base_url, "Tenant B").await;
    let client_b = McpClient::with_bearer(&server.base_url, &key_b);
    let qualified = format!("{ns_o}.lookup");

    let enqueue_result = client_b
        .tools_call(
            "host.tool_call",
            json!({"name": qualified, "args": {"msg": "hi"}, "async": true}),
        )
        .await
        .expect("B enqueues an async call on O's shared tool");
    let run_id = extract_structured(&enqueue_result)["run_id"]
        .as_str()
        .expect("run_id")
        .to_string();

    let wait_result = client_b
        .tools_call("host.runs.wait", json!({"run_id": run_id, "timeout_s": 10}))
        .await
        .expect("B waits on its own run");
    let wait_struct = extract_structured(&wait_result);
    assert_eq!(wait_struct["status"], json!("done"), "run must finish done: {wait_struct:?}");
    assert_eq!(wait_struct["result"], json!({"msg": "hi"}));

    let owner_get_err = client_o
        .tools_call("host.runs.get", json!({"run_id": run_id}))
        .await
        .expect_err("the owner must never see the caller's own run");
    assert_eq!(owner_get_err.error_code.as_deref(), Some("run_not_found"));
}
