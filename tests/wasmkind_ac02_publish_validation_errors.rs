//! PRD-mcphost-wasm-kind
//! AC2 -- Given a core module that is not a component, and a component over
//! the size cap, When published, Then each is refused with a structured
//! error naming the requirement or bound.

use crate::common;
use common::{TestServer, signup, wasm_kind_registry};
use serde_json::json;

/// The minimal valid core WebAssembly module: just the `\0asm` magic and
/// version 1, no sections -- accepted by `wasmtime::Module::new`, refused
/// by `wasmtime::component::Component::new` (a different binary format,
/// not merely a stricter validator).
const EMPTY_CORE_MODULE: &[u8] = &[0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00];

#[tokio::test]
async fn core_module_is_refused_naming_component_model() {
    use base64::Engine as _;
    let server = TestServer::start_with_kinds(wasm_kind_registry()).await;
    let (_ns, key) = signup(&server.base_url, "Wasm AC2 Tenant A").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    let component_b64 = base64::engine::general_purpose::STANDARD.encode(EMPTY_CORE_MODULE);
    let err = client
        .tools_call(
            "host.tool_publish",
            json!({
                "name": "not_a_component",
                "kind": "wasm",
                "spec": {"component": component_b64},
            }),
        )
        .await
        .expect_err("a core module must be refused");

    assert_eq!(err.error_code.as_deref(), Some("invalid_spec"));
    assert!(
        err.message.contains("component model"),
        "message must name the component-model requirement: {}",
        err.message
    );
    assert_eq!(
        err.data.get("field").and_then(|v| v.as_str()),
        Some("component"),
        "data: {}",
        err.data
    );
}

#[tokio::test]
async fn oversize_component_is_refused_naming_the_bound() {
    use base64::Engine as _;
    let server = TestServer::start_with_kinds(wasm_kind_registry()).await;
    let (_ns, key) = signup(&server.base_url, "Wasm AC2 Tenant B").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    // One byte over this kind's 32 KiB cap -- doesn't need to be a real
    // component at all, since the size check runs before any component-model
    // parsing.
    let oversized = vec![0u8; 32 * 1024 + 1];
    let component_b64 = base64::engine::general_purpose::STANDARD.encode(&oversized);
    let err = client
        .tools_call(
            "host.tool_publish",
            json!({
                "name": "too_big",
                "kind": "wasm",
                "spec": {"component": component_b64},
            }),
        )
        .await
        .expect_err("an oversize component must be refused");

    assert_eq!(err.error_code.as_deref(), Some("invalid_spec"));
    assert!(
        err.message.contains("32768"),
        "message must name the byte bound: {}",
        err.message
    );
    assert_eq!(
        err.data.get("field").and_then(|v| v.as_str()),
        Some("component"),
        "data: {}",
        err.data
    );
}
