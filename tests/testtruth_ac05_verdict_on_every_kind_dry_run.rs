//! PRD-mcphost-tool-test-truth AC5 (P0) — Given an `echo` tool, When
//! `host.tool_test` runs, Then `verdict: pass` and the contract test
//! asserts `verdict` is present on every kind's dry-run response.

use crate::common;
use common::{McpClient, TestServer, publish, signup};
use mcphost::kinds::KindRegistry;
use mcphost::kinds::chain::ChainKind;
use mcphost::kinds::http::{HttpKind, NameLookup};
use mcphost::kinds::python::PythonKind;
use mcphost::sandbox;
use serde_json::json;
use std::collections::HashMap;
use std::sync::Arc;
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn echo_tool_dry_run_is_pass() {
    let server = TestServer::start_with_kinds(KindRegistry::with_builtin()).await;
    let (_ns, key) = signup(&server.base_url, "Tool Test Truth AC5 Echo Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    publish(&client, "echoer", "echo", json!({"schema": {"type": "object"}})).await;

    let result = client
        .tools_call("host.tool_test", json!({"name": "echoer", "args": {}}))
        .await
        .expect("host.tool_test must succeed");
    let structured = common::extract_structured(&result);

    assert_eq!(structured["verdict"], json!("pass"), "{structured}");
}

/// Requirement 1's own contract: `verdict` is top-level on every kind's
/// `host.tool_test` response, not just `chain`'s -- `echo`/`http` need no
/// sandbox and always run; `python` is skipped on a host with no
/// unprivileged user namespaces, same as every other python-kind test in
/// this suite.
#[tokio::test]
async fn every_kind_dry_run_carries_a_verdict_field() {
    let upstream = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ok": true})))
        .mount(&upstream)
        .await;

    let envs_dir = common::TempDataDir::new();
    let mut kinds = KindRegistry::with_builtin();
    kinds.register(Arc::new(ChainKind));
    let lookup: Arc<dyn NameLookup> = Arc::new(common::FixedLookup(HashMap::new()));
    kinds.register(Arc::new(HttpKind::for_test("127.0.0.1", lookup)));
    kinds.register(Arc::new(PythonKind::new(&envs_dir.0)));

    let server = TestServer::start_with_kinds(kinds).await;
    let (_ns, key) = signup(&server.base_url, "Tool Test Truth AC5 Contract Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    publish(&client, "k_echo", "echo", json!({"schema": {"type": "object"}})).await;
    publish(
        &client,
        "k_http",
        "http",
        json!({"method": "GET", "url": format!("{}/x", upstream.uri()), "args_schema": {"type": "object"}}),
    )
    .await;
    publish(
        &client,
        "k_chain",
        "chain",
        json!({"steps": [{"tool": "k_echo", "args": {}}]}),
    )
    .await;

    for name in ["k_echo", "k_http", "k_chain"] {
        let result = client
            .tools_call("host.tool_test", json!({"name": name, "args": {}}))
            .await
            .unwrap_or_else(|e| panic!("host.tool_test({name}) must succeed: {} {}", e.code, e.message));
        let structured = common::extract_structured(&result);
        assert!(
            structured.get("verdict").is_some(),
            "{name}'s host.tool_test response must carry a verdict field: {structured}"
        );
    }

    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    publish(
        &client,
        "k_python",
        "python",
        json!({"source": "def main(args):\n    return {}\n", "network": "none"}),
    )
    .await;
    let result = common::poll_until_ready(
        &client,
        "host.tool_test",
        json!({"name": "k_python", "args": {}}),
        std::time::Duration::from_secs(15),
    )
    .await
    .unwrap_or_else(|e| panic!("host.tool_test(k_python) must succeed: {} {}", e.code, e.message));
    let structured = common::extract_structured(&result);
    assert!(
        structured.get("verdict").is_some(),
        "k_python's host.tool_test response must carry a verdict field: {structured}"
    );
}
