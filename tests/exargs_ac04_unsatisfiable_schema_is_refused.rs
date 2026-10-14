//! PRD-mcphost-tool-test-examples-from-schema AC4 — a schema the generator
//! cannot satisfy (`minimum: 10, maximum: 5`), tested without args, answers
//! `example_unavailable` naming `/n` and starts no run.

use crate::common;
use common::{McpClient, TestServer, signup};
use mcphost::kinds::KindRegistry;
use mcphost::kinds::example::checked_example;
use serde_json::json;

#[test]
fn generator_reports_the_failing_path() {
    let err = checked_example(&json!({"type": "object", "required": ["n"], "properties": {"n": {"type": "integer", "minimum": 10, "maximum": 5}}}))
        .expect_err("unsatisfiable");
    assert_eq!(err.path, "/n");
}

#[tokio::test]
async fn tool_test_without_args_refuses_and_runs_nothing() {
    let server = TestServer::start_with_kinds(KindRegistry::with_builtin()).await;
    let (_ns, key) = signup(&server.base_url, "Exargs AC4").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    client
        .tools_call(
            "host.tool_publish",
            json!({
                "name": "impossible",
                "kind": "echo",
                "spec": {"schema": {"type": "object", "required": ["n"], "properties": {"n": {"type": "integer", "minimum": 10, "maximum": 5}}}},
            }),
        )
        .await
        .expect("publish impossible");

    let err = client.tools_call("host.tool_test", json!({"name": "impossible"})).await.expect_err("refused");
    assert_eq!(err.error_code.as_deref(), Some("example_unavailable"), "{err:?}");
    assert!(err.message.contains("/n"), "{}", err.message);
    assert_eq!(err.data["path"], json!("/n"));
}
