//! PRD-mcphost-args-invalid-message-from-data
//! AC1 — Given a tool with `qty: integer`, When called with
//! `{"qty": 4.78}`, Then code is `args_invalid`, the message names the field,
//! both types, the received value and that nothing is coerced, and `data`
//! carries `field`, `got` and `docs`.

use crate::common;
use common::{McpClient, TestServer, signup};
use mcphost::kinds::KindRegistry;
use serde_json::json;

#[tokio::test]
async fn float_for_integer_message_is_exact_and_data_has_field_got_docs() {
    let server = TestServer::start_with_kinds(KindRegistry::with_builtin()).await;
    let (ns, key) = signup(&server.base_url, "ArgMsg AC1").await;
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

    let err = client
        .tools_call(&format!("{ns}.qty_tool"), json!({"qty": 4.78}))
        .await
        .expect_err("4.78 is not an integer");

    assert_eq!(err.error_code.as_deref(), Some("args_invalid"));
    assert_eq!(
        err.message,
        "args.qty: expected integer, got number (4.78); no coercion is applied — send an integer or change the schema"
    );
    assert_eq!(err.data["field"], json!("qty"), "data: {:?}", err.data);
    assert_eq!(err.data["got"], json!("4.78"));
    assert_eq!(err.data["docs"], json!("host.tool_test"));
}
