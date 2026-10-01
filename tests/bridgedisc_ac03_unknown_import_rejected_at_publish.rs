//! PRD-mcphost-sandbox-bridge-discoverability
//! AC3 (P0) -- Given a python spec whose source contains `import host`,
//! When `host.spec_test` or `host.tool_publish` runs, Then the error class
//! is `unknown_import`, `data.hint` contains `import mcphost` and all
//! three module names, and nothing is published.
//!
//! Both gates run this exact scan before ever spinning up the sandbox
//! (`validate_spec_fields_all`, before the async `ast_check`), so this
//! test needs no `require_user_namespaces_or_ci_skip` guard -- it runs
//! everywhere, including hosted CI.

use crate::common;
use common::{McpClient, TestServer, python_kind_registry, signup};
use serde_json::json;

const BAD_SOURCE: &str = "import host\n\ndef main(args):\n    return host.state.get(\"x\")\n";

fn assert_unknown_import(err: &common::RpcError, context: &str) {
    assert_eq!(
        err.error_code.as_deref(),
        Some("unknown_import"),
        "{context} must fail unknown_import, got {err:?}"
    );
    let hint = err.data["hint"]
        .as_str()
        .unwrap_or_else(|| panic!("{context}: data.hint missing: {err:?}"));
    assert!(hint.contains("import mcphost"), "{context}: hint must name the import line: {hint}");
    for module in ["mcphost.state", "mcphost.table", "mcphost.docs"] {
        assert!(hint.contains(module), "{context}: hint must name {module}: {hint}");
    }
    assert_eq!(err.data["module"], json!("host"), "{context}: data.module must name the bad import: {err:?}");
}

#[tokio::test]
async fn import_host_rejected_by_tool_publish() {
    let envs_dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (ns, key) = signup(&server.base_url, "Bridge AC3 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let err = client
        .tools_call(
            "host.tool_publish",
            json!({"name": "bad_import", "kind": "python", "spec": {"source": BAD_SOURCE}}),
        )
        .await
        .expect_err("import host must be rejected at publish, before any sandbox runs");
    assert_unknown_import(&err, "host.tool_publish");

    // Nothing is published: the qualified name must not resolve.
    let listed = client
        .tools_call("host.tool_list", json!({}))
        .await
        .expect("host.tool_list");
    let structured = common::extract_structured(&listed);
    let names: Vec<&str> = structured["tools"]
        .as_array()
        .map(|t| t.iter().filter_map(|v| v["name"].as_str()).collect())
        .unwrap_or_default();
    assert!(
        !names.iter().any(|n| n.contains("bad_import")),
        "a rejected publish must leave nothing behind: {names:?} (namespace {ns})"
    );
}

#[tokio::test]
async fn import_host_rejected_by_spec_test() {
    let envs_dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (_ns, key) = signup(&server.base_url, "Bridge AC3 Spec Test Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let err = client
        .tools_call(
            "host.spec_test",
            json!({
                "kind": "python",
                "spec": {"source": BAD_SOURCE},
                "invocations": [{}],
            }),
        )
        .await
        .expect_err("import host must be rejected by host.spec_test too");
    assert_unknown_import(&err, "host.spec_test");
}
