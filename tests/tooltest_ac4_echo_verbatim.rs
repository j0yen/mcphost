//! PRD-mcphost-tool-test AC4 — Given an `echo` spec, When tested, Then each
//! invocation returns its arguments verbatim.

mod common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn echo_invocations_return_arguments_verbatim() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Echo Spec Test Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let spec = json!({"schema": {"type": "object", "properties": {"msg": {"type": "string"}}, "required": ["msg"]}});
    let result = client
        .tools_call(
            "host.spec_test",
            json!({
                "kind": "echo",
                "spec": spec,
                "invocations": [{"msg": "hello"}, {"msg": "world"}],
            }),
        )
        .await
        .expect("spec_test ok");
    let structured = extract_structured(&result);
    let invocations = structured["invocations"].as_array().expect("array");
    assert_eq!(invocations[0]["output"], json!({"msg": "hello"}));
    assert_eq!(invocations[1]["output"], json!({"msg": "world"}));
}
