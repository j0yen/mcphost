//! PRD-mcphost-schema-coercion-opt-in AC3 -- Given the opted-in integer
//! property, When called with `4.78`, `"4.5"`, `" 4"`, `"yes"` or `"1e3"`,
//! Then each is `args_invalid` with `expected_type: integer`, the original
//! `actual_type` and `coercion_tried`; nothing is rounded or trimmed.

use crate::common;
use common::{McpClient, TestServer, signup};
use mcphost::kinds::KindRegistry;
use serde_json::json;

#[tokio::test]
async fn lossy_values_are_refused_with_coercion_tried() {
    let server = TestServer::start_with_kinds(KindRegistry::with_builtin()).await;
    let (ns, key) = signup(&server.base_url, "Coerce AC3").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "opt_in", "kind": "echo", "spec": {"schema": {
                "type": "object", "properties": {"n": {"type": "integer", "x-coerce": true}}}}}),
        )
        .await
        .expect("publish");

    let cases = [
        (json!(4.78), "number", "number → integer"),
        (json!("4.5"), "string", "string → integer"),
        (json!(" 4"), "string", "string → integer"),
        (json!("yes"), "string", "string → integer"),
        (json!("1e3"), "string", "string → integer"),
    ];
    for (sent, actual, tried) in cases {
        let err = client
            .tools_call(&format!("{ns}.opt_in"), json!({"n": sent.clone()}))
            .await
            .expect_err(&format!("{sent} must be refused"));
        assert_eq!(err.error_code.as_deref(), Some("args_invalid"), "{sent}");
        assert_eq!(err.data["phase"], json!("args_coercion"), "{sent}");
        assert_eq!(err.data["expected_type"], json!("integer"), "{sent}");
        assert_eq!(err.data["actual_type"], json!(actual), "{sent}");
        assert_eq!(err.data["coercion_tried"], json!(tried), "{sent}: {:?}", err.data);
        assert_eq!(err.data["got"].as_str(), Some(serde_json::to_string(&sent).unwrap().as_str()), "value untouched");
    }
}
