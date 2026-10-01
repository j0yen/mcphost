//! PRD-mcphost-sandbox-bridge-discoverability
//! AC4 (P0) -- Given a published python tool whose source does `import
//! mcphost_sdk` inside a function (escaping the static scan), When it is
//! called, Then the call fails `unknown_import` with the same hint and no
//! raw traceback appears in `result`.
//!
//! A literal `import mcphost_sdk` statement, even indented inside a
//! function body, is still caught by requirement 3's line-based scan (it
//! checks every line, not just module level) -- so publishing one would
//! never reach this test's "When it is called" step at all.
//! `importlib.import_module("mcphost_sdk")` is the dynamic form that
//! actually escapes a line-based text scanner (the technical
//! considerations' own scoped decision: "Dynamic imports are out of
//! scope" for the scan) while still raising the identical
//! `ModuleNotFoundError: No module named 'mcphost_sdk'` at call time --
//! caught by `PY_RUNNER_SCRIPT`'s own `except Exception` into a normal
//! envelope, which `map_envelope_error`'s new check (not the uncaught-crash
//! path `module_not_found_hint` handles) must still map to `unknown_import`.

use crate::common;
use common::{McpClient, TestServer, python_kind_registry, signup};
use mcphost::sandbox;
use serde_json::json;

const DYNAMIC_IMPORT_SOURCE: &str =
    "import importlib\n\ndef main(args):\n    mod = importlib.import_module(\"mcphost_sdk\")\n    return mod.anything()\n";

#[tokio::test]
async fn dynamic_mcphost_sdk_import_fails_unknown_import_no_traceback() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs_dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (_ns, key) = signup(&server.base_url, "Bridge AC4 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    // The dynamic form publishes cleanly: nothing in the static scan or
    // the sandboxed AST check can see a problem with it.
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "dynamic_import", "kind": "python", "spec": {"source": DYNAMIC_IMPORT_SOURCE}}),
        )
        .await
        .expect("a dynamic import must publish -- it has no detectable problem until it runs");

    let err = client
        .tools_call("host.tool_test", json!({"name": "dynamic_import", "args": {}}))
        .await
        .expect_err("calling the published tool must fail once the dynamic import actually runs");

    assert_eq!(
        err.error_code.as_deref(),
        Some("unknown_import"),
        "a ModuleNotFoundError raised inside main must map to unknown_import, got {err:?}"
    );
    let hint = err.data["hint"].as_str().expect("data.hint present");
    assert!(hint.contains("import mcphost"), "hint must name the import line: {hint}");
    assert_eq!(err.data["module"], json!("mcphost_sdk"), "{err:?}");

    // "no raw traceback appears in result": unknown_import's own data
    // carries only hint/module, never the tool's traceback/stdout/stderr.
    assert!(err.data.get("traceback").is_none(), "{err:?}");
    assert!(err.data.get("stderr_tail").is_none(), "{err:?}");
    assert!(err.data.get("stdout_tail").is_none(), "{err:?}");
}
