//! PRD-mcphost-tool-test-examples-from-schema AC5 — `host.tool_publish` with
//! `dry_run: true` and no `invocations`, and `host.spec_test` with no
//! `invocations`, each run exactly one generated example and report it under
//! `example_args`.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use mcphost::kinds::KindRegistry;
use serde_json::json;

fn spec() -> serde_json::Value {
    json!({"schema": {"type": "object", "properties": {"n": {"type": "integer", "default": 2}}}})
}

#[tokio::test]
async fn spec_test_without_invocations_runs_one_example() {
    let server = TestServer::start_with_kinds(KindRegistry::with_builtin()).await;
    let (_ns, key) = signup(&server.base_url, "Exargs AC5a").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    let out = extract_structured(
        &client.tools_call("host.spec_test", json!({"kind": "echo", "spec": spec()})).await.expect("spec_test"),
    );
    assert_eq!(out["example_args"], json!({"n": 2}));
    assert_eq!(out["untyped"], json!([]));
    let ran = out["invocations"].as_array().expect("invocations");
    assert_eq!(ran.len(), 1, "exactly one example runs: {out}");
}

#[tokio::test]
async fn publish_dry_run_without_invocations_runs_one_example_and_publishes_nothing() {
    let server = TestServer::start_with_kinds(KindRegistry::with_builtin()).await;
    let (_ns, key) = signup(&server.base_url, "Exargs AC5b").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    let out = extract_structured(
        &client
            .tools_call("host.tool_publish", json!({"name": "dry", "kind": "echo", "spec": spec(), "dry_run": true}))
            .await
            .expect("dry-run publish"),
    );
    assert_eq!(out["ok"], json!(true), "{out}");
    assert_eq!(out["example_args"], json!({"n": 2}));
    assert_eq!(out["invocations"].as_array().expect("invocations").len(), 1, "{out}");

    let err = client.tools_call("host.tool_test", json!({"name": "dry"})).await.expect_err("nothing was published");
    assert_eq!(err.error_code.as_deref(), Some("tool_not_found"), "{err:?}");
}
