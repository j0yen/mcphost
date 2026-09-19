//! PRD-mcphost-wasm-kind
//! AC5 -- Given a wasm spec, When `host.tool_test` runs, Then the dry-run
//! executes the component with the rendered arguments and returns the same
//! test surface shape other kinds return.

use crate::common;
use common::{TestServer, extract_structured, signup, wasm_fixture_b64, wasm_kind_registry};
use serde_json::json;

#[tokio::test]
async fn tool_test_runs_the_component_and_returns_the_shared_surface_shape() {
    let server = TestServer::start_with_kinds(wasm_kind_registry()).await;
    let (_ns, key) = signup(&server.base_url, "Wasm AC5 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call(
            "host.tool_publish",
            json!({
                "name": "echoer",
                "kind": "wasm",
                "spec": {
                    "component": wasm_fixture_b64("echo"),
                    "outputs": ["msg"],
                },
            }),
        )
        .await
        .expect("publish wasm echo tool");

    let result = client
        .tools_call(
            "host.tool_test",
            json!({"name": "echoer", "args": {"msg": "dry run"}}),
        )
        .await
        .expect("host.tool_test must succeed");
    let structured = extract_structured(&result);

    // The same shape other kinds' host.tool_test returns: the real call's
    // own payload, a declared-outputs envelope report (since this spec
    // declares outputs), and a state tally -- none of this is wasm-specific
    // code, it all falls out of implementing the shared Kind trait.
    assert_eq!(
        structured["payload"],
        json!({"msg": "dry run"}),
        "dry run must execute the component with the rendered arguments: {structured}"
    );
    assert_eq!(
        structured["envelope"]["green"],
        json!(true),
        "declared output 'msg' must be found at the contract path: {structured}"
    );
    assert_eq!(structured["envelope"]["missing"], json!([]));
    assert!(
        structured.get("state").is_some(),
        "must carry the same state tally other kinds' tool_test responses do: {structured}"
    );

    // A no tool row is written -- host.tool_test never meters (AC1's own
    // metering test proves a real call does).
    let usage = client
        .tools_call("host.usage", json!({"window": "24h"}))
        .await
        .expect("host.usage");
    assert_eq!(extract_structured(&usage)["calls"].as_i64(), Some(0));
}
