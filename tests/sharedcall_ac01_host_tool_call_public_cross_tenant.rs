//! PRD-mcphost-shared-tool-call-path
//! AC1 — Given owner O publishes `lookup` and shares it public, When tenant
//! B calls `host.tool_call {name: "<O>.lookup", args}`, Then B receives the
//! tool's result and a meter event is written for O's tool with B as
//! caller.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn host_tool_call_with_qualified_name_reaches_the_shared_tool() {
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

    let (ns_b, key_b) = signup(&server.base_url, "Tenant B").await;
    let client_b = McpClient::with_bearer(&server.base_url, &key_b);
    let qualified = format!("{ns_o}.lookup");
    let call = client_b
        .tools_call("host.tool_call", json!({"name": qualified, "args": {"msg": "hi"}}))
        .await
        .expect("B calls O's shared tool via host.tool_call");
    let result = extract_structured(&call);
    assert_eq!(result["msg"], json!("hi"));

    let tenant_o = server
        .state
        .db
        .find_tenant_by_namespace(ns_o.clone())
        .await
        .expect("find O")
        .expect("O exists");
    let by_others = server
        .state
        .db
        .calls_by_others(tenant_o.id, 86_400)
        .await
        .expect("calls_by_others");
    assert!(
        by_others.iter().any(|(ns, n)| ns == &ns_b && *n == 1),
        "O's calls_by_others must show B: 1, got {by_others:?}"
    );
}
