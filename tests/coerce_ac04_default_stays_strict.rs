//! PRD-mcphost-schema-coercion-opt-in AC4 -- Given `{"n": {"type":
//! "integer"}}` (no opt-in), When called with `"4"`, Then `args_invalid`
//! exactly as before (no `coercion_tried`, no `coerced`), while the same
//! call on an opted-in twin runs -- so the test fails without the feature
//! and without the strict default.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use mcphost::kinds::KindRegistry;
use serde_json::json;

#[tokio::test]
async fn no_opt_in_refuses_the_same_value_an_opted_in_twin_accepts() {
    let server = TestServer::start_with_kinds(KindRegistry::with_builtin()).await;
    let (ns, key) = signup(&server.base_url, "Coerce AC4").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    for (name, opt) in [("strict", json!({"type": "integer"})), ("loose", json!({"type": "integer", "x-coerce": true})), ("off", json!({"type": "integer", "x-coerce": false}))] {
        client
            .tools_call(
                "host.tool_publish",
                json!({"name": name, "kind": "echo", "spec": {"schema": {"type": "object", "properties": {"n": opt}}}}),
            )
            .await
            .expect("publish");
    }

    let ok = extract_structured(&client.tools_call(&format!("{ns}.loose"), json!({"n": "4"})).await.expect("opted in runs"));
    assert_eq!(ok["n"], json!(4));

    for name in ["strict", "off"] {
        let err = client.tools_call(&format!("{ns}.{name}"), json!({"n": "4"})).await.expect_err("strict by default");
        assert_eq!(err.error_code.as_deref(), Some("args_invalid"), "{name}");
        assert_eq!(err.data["phase"], json!("args_coercion"));
        assert_eq!(err.data["expected_type"], json!("integer"));
        assert_eq!(err.data["actual_type"], json!("string"));
        assert_eq!(err.data["got"], json!("\"4\""));
        assert!(err.data.get("coercion_tried").is_none(), "{name}: no attempt without opt-in: {:?}", err.data);
    }
}
