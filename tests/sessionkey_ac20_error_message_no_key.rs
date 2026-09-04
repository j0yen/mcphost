//! PRD-mcphost-session-key
//! AC20 — Given an error path such as invalid arguments on a call that
//! carried `tenant_key`, When the error message is returned to the caller,
//! Then it contains no part of the key.

mod common;
use common::{McpClient, TestServer, signup};
use serde_json::json;

#[tokio::test]
async fn args_invalid_error_never_echoes_the_tenant_key_that_authenticated_the_call() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Error Path Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call(
            "host.tool_publish",
            json!({
                "name": "hello",
                "kind": "echo",
                "spec": {"schema": {"type": "object", "properties": {"msg": {"type": "string"}}, "required": ["msg"]}},
            }),
        )
        .await
        .expect("publish");

    // No Authorization header at all -- tenant_key is the only credential
    // on this call, and it is deliberately paired with args that violate
    // the published tool's schema.
    let anon = McpClient::new(&server.base_url);
    let err = anon
        .tools_call(
            "host.tool_call",
            json!({"name": "hello", "args": {"nope": 1}, "tenant_key": key}),
        )
        .await
        .expect_err("invalid args must fail");
    assert_eq!(err.error_code.as_deref(), Some("args_invalid"));

    assert!(
        !err.message.contains(&key),
        "the args_invalid error message must not contain the tenant_key: {}",
        err.message
    );
    let data_dump = err.data.to_string();
    assert!(
        !data_dump.contains(&key),
        "the error's data payload must not contain the tenant_key either: {data_dump}"
    );
}
