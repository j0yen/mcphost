//! PRD-mcphost-sandbox-bridge-discoverability
//! AC4 (P0) -- Given a published python tool whose source does `import
//! mcphost_sdk` inside a function (escaping the static scan), When it is
//! called, Then the call fails `unknown_import` with the same hint and no
//! raw traceback appears in `result`.

use crate::common;
use common::{McpClient, TestServer, poll_until_ready, python_kind_registry, signup};
use mcphost::sandbox;
use serde_json::json;
use std::time::Duration;

/// `importlib.import_module("mcphost_sdk")`, not a literal `import
/// mcphost_sdk` line -- requirement 3's static scan (AC3) matches literal
/// import-statement text only ("dynamic imports are out of scope" per its
/// own requirement text), so this source publishes cleanly and only fails
/// once the dynamic import actually runs, inside `main(args)`.
const SOURCE: &str = "import importlib\n\n\ndef main(args):\n    importlib.import_module(\"mcphost_sdk\")\n    return {\"ok\": True}\n";

#[tokio::test]
async fn dynamic_mcphost_sdk_import_fails_unknown_import_with_no_raw_traceback() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs_dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (ns, key) = signup(&server.base_url, "Bridgedisc AC4 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "escapes_static_scan", "kind": "python", "spec": {"source": SOURCE}}),
        )
        .await
        .expect("a dynamically-constructed import must publish cleanly (it escapes the static scan)");

    let qualified = format!("{ns}.escapes_static_scan");
    let err = poll_until_ready(&client, &qualified, json!({}), Duration::from_secs(10))
        .await
        .expect_err("importing a module that does not exist must fail the call");

    assert_eq!(
        err.data["error_code"], json!("unknown_import"),
        "the dynamic import must be mapped to the same unknown_import class a rejected publish gets: {err:?}"
    );
    let hint = err.data["hint"].as_str().expect("data.hint is a string");
    assert!(hint.contains("import mcphost"), "hint: {hint}");
    assert!(hint.contains("mcphost.state"), "hint: {hint}");
    assert!(hint.contains("mcphost.table"), "hint: {hint}");
    assert!(hint.contains("mcphost.docs"), "hint: {hint}");

    // No raw traceback anywhere in the error: neither a literal "Traceback"
    // line nor the runner's own `traceback`/`stderr_tail` data fields.
    let whole_err = err.data.to_string() + &err.message;
    assert!(
        !whole_err.contains("Traceback (most recent call last)"),
        "no raw CPython traceback may reach the caller: {whole_err}"
    );
    assert!(err.data.get("traceback").is_none(), "data must carry no traceback field: {:?}", err.data);
    assert!(err.data.get("stderr_tail").is_none(), "data must carry no stderr_tail field: {:?}", err.data);
}
