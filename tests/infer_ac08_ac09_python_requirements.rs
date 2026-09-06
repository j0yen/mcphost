//! PRD-mcphost-tool-infer AC8/AC9 (P0) — Given a `python` source with no
//! `requirements`, When it is published, Then a standard-library-only
//! import needs nothing listed (AC8), and an import present in the
//! import-to-distribution map is installed and callable without the agent
//! listing it (AC9).

mod common;
use common::{TestServer, extract_structured, poll_until_ready, python_kind_registry, signup};
use mcphost::sandbox;
use serde_json::json;
use std::time::Duration;

#[tokio::test]
async fn stdlib_only_import_needs_no_requirements_listed() {
    // Requirement 8/9: this test builds and runs a real python-kind tool
    // via the sandbox, which needs unprivileged user namespaces. Not
    // guaranteed on GitHub's hosted runners, so skip cleanly in CI (and
    // fail loudly, not skip, anywhere else -- see
    // require_user_namespaces_or_ci_skip's doc comment) rather than fail
    // with "the tool's environment failed to build" -- same pattern as the
    // sandbox-dependent unit tests in src/kinds/python.rs and
    // src/sandbox.rs, and as tests/ac17_kind_conformance.rs.
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs_dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (ns, key) = signup(&server.base_url, "AC8 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    let spec = json!({
        "source": "import json\n\ndef main(args):\n    return {\"dumped\": json.dumps({\"ok\": True})}\n",
    });
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "stdlib_only", "kind": "python", "spec": spec}),
        )
        .await
        .expect("publish with a stdlib-only import and no requirements must succeed");

    let result = poll_until_ready(
        &client,
        &format!("{ns}.stdlib_only"),
        json!({}),
        Duration::from_secs(60),
    )
    .await
    .unwrap_or_else(|e| panic!("call must succeed with no requirements listed: {} {}", e.code, e.message));
    assert_eq!(extract_structured(&result)["dumped"], "{\"ok\": true}");
}

#[tokio::test]
async fn mapped_import_is_installed_and_callable_without_requirements_listed() {
    // Requirement 8/9: this test builds and runs a real python-kind tool
    // via the sandbox, which needs unprivileged user namespaces. Not
    // guaranteed on GitHub's hosted runners, so skip cleanly in CI (and
    // fail loudly, not skip, anywhere else -- see
    // require_user_namespaces_or_ci_skip's doc comment) rather than fail
    // with "the tool's environment failed to build" -- same pattern as the
    // sandbox-dependent unit tests in src/kinds/python.rs and
    // src/sandbox.rs, and as tests/ac17_kind_conformance.rs.
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs_dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (ns, key) = signup(&server.base_url, "AC9 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    let spec = json!({
        "source": "import httpx\n\ndef main(args):\n    return {\"has_get\": hasattr(httpx, \"get\")}\n",
    });
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "mapped_dep", "kind": "python", "spec": spec}),
        )
        .await
        .expect("publish with a mapped import and no requirements must succeed");

    let result = poll_until_ready(
        &client,
        &format!("{ns}.mapped_dep"),
        json!({}),
        Duration::from_secs(90),
    )
    .await
    .unwrap_or_else(|e| panic!("call must succeed once the mapped dependency builds: {} {}", e.code, e.message));
    assert!(extract_structured(&result)["has_get"].as_bool().unwrap_or(false));
}
