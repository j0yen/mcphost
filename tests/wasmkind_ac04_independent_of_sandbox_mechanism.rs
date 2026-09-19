//! PRD-mcphost-wasm-kind
//! AC4 -- Given the sandbox mechanism is reported unavailable (bwrap
//! absent), When a wasm tool is called, Then it succeeds, and a python call
//! on the same host still fails with `sandbox_unavailable` -- proving
//! independence.

use crate::common;
use common::{fake_interpreter_failing, signup, wasm_fixture_b64};
use mcphost::kinds::KindRegistry;
use mcphost::kinds::python::PythonKind;
use mcphost::kinds::wasm::WasmKind;
use mcphost::sandbox;
use serde_json::json;
use std::sync::Arc;

/// `echo` (base) + a `wasm` kind (no OS dependency at all) + a `python`
/// kind whose sandbox self-test has already run and found unready -- same
/// shape as `sandboxready_ac3_ac4_publish_rejection.rs`'s own
/// `unready_registry`, with `wasm` added.
async fn registry_with_unready_python(data_dir: &std::path::Path) -> KindRegistry {
    let mut kinds = KindRegistry::with_builtin();
    kinds.register(Arc::new(WasmKind::new()));
    let py = Arc::new(PythonKind::for_test_with_selftest(data_dir, 300));
    py.set_selftest_interpreter_for_test(Some(&fake_interpreter_failing(
        "bwrap: loopback: Failed RTM_NEWADDR: Operation not permitted",
    )));
    py.run_startup_selftest().await;
    kinds.register(py);
    kinds
}

#[tokio::test]
async fn wasm_call_succeeds_while_python_publish_fails_sandbox_unavailable() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let data_dir = common::TempDataDir::new();
    let kinds = registry_with_unready_python(&data_dir.0).await;
    let server = common::TestServer::start_with_kinds(kinds).await;
    let (ns, key) = signup(&server.base_url, "Wasm AC4 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    // The python kind's sandbox is unavailable on this host.
    let err = client
        .tools_call(
            "host.tool_publish",
            json!({
                "name": "my_py",
                "kind": "python",
                "spec": {"source": "def main(args):\n    return {}\n"},
            }),
        )
        .await
        .expect_err("python publish must be rejected while the sandbox is unready");
    assert_eq!(err.error_code.as_deref(), Some("sandbox_unavailable"));

    // A wasm tool never touches that mechanism at all, so it publishes and
    // calls exactly as it would on a fully capable host.
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
        .expect("wasm publish must succeed while python's sandbox is unready");
    let result = client
        .tools_call(&format!("{ns}.echoer"), json!({"msg": "independent"}))
        .await
        .expect("wasm call must succeed while python's sandbox is unready");
    assert_eq!(
        common::extract_structured(&result)["payload"],
        json!({"msg": "independent"})
    );
}
