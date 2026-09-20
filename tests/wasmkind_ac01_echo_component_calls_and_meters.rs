//! PRD-mcphost-wasm-kind
//! AC1 -- Given a valid component that echoes its arguments, When published
//! as `kind: wasm` and called, Then the call succeeds with the output under
//! `result.payload` and a metering row exists.

use crate::common;
use common::{TestServer, extract_structured, signup, wasm_fixture_b64, wasm_kind_registry};
use serde_json::json;

#[tokio::test]
async fn echo_component_call_succeeds_under_payload_and_is_metered() {
    let server = TestServer::start_with_kinds(wasm_kind_registry()).await;
    let (ns, key) = signup(&server.base_url, "Wasm AC1 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call(
            "host.tool_publish",
            json!({
                "name": "echoer",
                "kind": "wasm",
                "spec": {"component": wasm_fixture_b64("echo")},
            }),
        )
        .await
        .expect("publish wasm echo tool");

    let qualified = format!("{ns}.echoer");
    let result = client
        .tools_call(&qualified, json!({"msg": "hi"}))
        .await
        .expect("call must succeed");
    let structured = extract_structured(&result);
    assert_eq!(
        structured["payload"],
        json!({"msg": "hi"}),
        "echoed arguments must land under result.payload: {structured}"
    );

    let usage = client
        .tools_call("host.usage", json!({"window": "24h"}))
        .await
        .expect("host.usage");
    let usage = extract_structured(&usage);
    assert_eq!(
        usage["calls"].as_i64(),
        Some(1),
        "a metering row must exist for the call: {usage}"
    );
}
