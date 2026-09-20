//! PRD-mcphost-wasm-kind
//! AC6 -- Given a component that traps, When called, Then the trap message
//! is in `host.tool_logs` and the call error is structured, not a raw
//! panic.

use crate::common;
use common::{TestServer, extract_structured, signup, wasm_fixture_b64, wasm_kind_registry};
use serde_json::json;

#[tokio::test]
async fn trap_is_a_structured_error_and_its_message_reaches_tool_logs() {
    let server = TestServer::start_with_kinds(wasm_kind_registry()).await;
    let (ns, key) = signup(&server.base_url, "Wasm AC6 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call(
            "host.tool_publish",
            json!({
                "name": "trapper",
                "kind": "wasm",
                "spec": {"component": wasm_fixture_b64("trap")},
            }),
        )
        .await
        .expect("publish trap tool");

    let err = client
        .tools_call(&format!("{ns}.trapper"), json!({}))
        .await
        .expect_err("a trapping component must not return normally");
    assert_eq!(err.error_code.as_deref(), Some("tool_trapped"));
    assert!(
        !err.message.to_lowercase().contains("panic"),
        "the call error must be structured, not a raw panic message: {}",
        err.message
    );
    assert!(
        err.data.get("trap").and_then(|v| v.as_str()).is_some_and(|s| !s.is_empty()),
        "data.trap must carry the trap's own message: {}",
        err.data
    );

    let logs = client
        .tools_call("host.tool_logs", json!({"name": "trapper"}))
        .await
        .expect("host.tool_logs");
    let logs = extract_structured(&logs);
    let lines = logs["lines"].as_array().expect("lines array");
    assert!(
        lines.iter().any(|l| l.as_str().is_some_and(|s| s.contains("trap"))),
        "trap message must be in host.tool_logs: {lines:?}"
    );
}
