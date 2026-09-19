//! PRD-mcphost-wasm-kind
//! AC10 -- Given a wasm-kind tool declaring `diagnosis` that returns
//! `{"analysis":{"diagnosis":"…"}}`, When called, Then
//! `result.payload.diagnosis` is present with that value -- the exact same
//! fixture shape and assertions as
//! `tests/envelope_ac2_python_object_declared_output_promoted.rs`, proving
//! the wasm kind's declared-output promotion is identical to python's.

use crate::common;
use common::{TestServer, extract_structured, signup, wasm_fixture_b64, wasm_kind_registry};
use serde_json::json;

#[tokio::test]
async fn declared_field_nested_under_analysis_lands_at_result_payload() {
    let server = TestServer::start_with_kinds(wasm_kind_registry()).await;
    let (ns, key) = signup(&server.base_url, "Wasm AC10 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    // The `echo` fixture returns its call arguments verbatim (`Ok(args)`),
    // so calling it with the same `{"analysis": {"diagnosis": ...}}` shape
    // envelope_ac2's python fixture *returns* reproduces that fixture's
    // output for the promotion logic to work on -- there is no
    // server-side wasm compiler to author a bespoke component inline
    // (this PRD's own non-goal), so reusing echo is how a wasm tool
    // produces this same shape.
    let spec = json!({
        "component": wasm_fixture_b64("echo"),
        "outputs": ["diagnosis"],
    });
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "diagnoser", "kind": "wasm", "spec": spec}),
        )
        .await
        .expect("publish ok");

    let result = client
        .tools_call(
            &format!("{ns}.diagnoser"),
            json!({"analysis": {"diagnosis": "looks fine"}}),
        )
        .await
        .expect("call must succeed");
    let structured = extract_structured(&result);

    assert_eq!(
        structured["payload"]["diagnosis"], "looks fine",
        "result.payload.diagnosis must be present: {structured}"
    );
    // Existing top-level shape stays alongside payload (identical to
    // python's own AC2 test: `analysis` is a top-level sibling of
    // `payload`, not nested inside it).
    assert_eq!(structured["analysis"]["diagnosis"], "looks fine");
}
