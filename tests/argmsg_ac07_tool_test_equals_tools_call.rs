//! PRD-mcphost-args-invalid-message-from-data
//! AC7 — Given the same tool and args, When `host.tool_test` runs, Then its
//! `code`, `message` and `data` equal the `tools/call` response's.

use crate::common;
use common::{McpClient, TestServer, signup};
use mcphost::kinds::KindRegistry;
use serde_json::json;

#[tokio::test]
async fn tool_test_args_invalid_equals_tools_call_args_invalid() {
    let server = TestServer::start_with_kinds(KindRegistry::with_builtin()).await;
    let (ns, key) = signup(&server.base_url, "ArgMsg AC7").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    client
        .tools_call(
            "host.tool_publish",
            json!({
                "name": "qty_tool",
                "kind": "echo",
                "spec": {"schema": {
                    "type": "object",
                    "properties": {"qty": {"type": "integer"}},
                }},
            }),
        )
        .await
        .expect("publish qty_tool");

    let args = json!({"qty": 4.78});
    let called = client
        .tools_call(&format!("{ns}.qty_tool"), args.clone())
        .await
        .expect_err("tools/call rejects 4.78");
    let tested = client
        .tools_call("host.tool_test", json!({"name": "qty_tool", "args": args}))
        .await
        .expect_err("host.tool_test rejects 4.78");

    assert_eq!(called.error_code.as_deref(), Some("args_invalid"));
    assert_eq!(tested.error_code, called.error_code);
    assert_eq!(tested.message, called.message);
    // `request_id` is per-response correlation, not part of the shape.
    let strip = |mut d: serde_json::Value| {
        if let Some(o) = d.as_object_mut() {
            o.remove("request_id");
        }
        d
    };
    assert_eq!(strip(tested.data.clone()), strip(called.data.clone()));
}
