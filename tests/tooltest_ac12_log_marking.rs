//! PRD-mcphost-tool-test AC12 — Given a tenant that ran tests, When it reads
//! its logs through the existing log tool, Then test invocations appear
//! marked as tests and are distinguishable from production calls.

use crate::common;
use common::{McpClient, TestServer, extract_structured, http_kind_registry, signup};
use serde_json::json;
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn spec_test_invocations_appear_marked_in_host_tool_logs() {
    let upstream = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ok": true})))
        .mount(&upstream)
        .await;

    let server = TestServer::start_with_kinds(http_kind_registry()).await;
    let (_ns, key) = signup(&server.base_url, "Log Marking Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let spec = json!({
        "method": "GET",
        "url": format!("{}/v1/thing", upstream.uri()),
        "args_schema": {"type": "object"},
    });
    client
        .tools_call(
            "host.spec_test",
            json!({"kind": "http", "spec": spec, "invocations": [{}]}),
        )
        .await
        .expect("spec_test ok");

    // Read back through `host.tool_logs` -- the "existing log tool" the AC
    // refers to -- under the per-kind synthetic bucket name, since
    // `host.spec_test` never publishes a `tools` row for a real name.
    let logs = extract_structured(
        &client
            .tools_call("host.tool_logs", json!({"name": "__spec_test__.http"}))
            .await
            .expect("host.tool_logs must serve the spec_test bucket"),
    );
    let lines = logs["lines"].as_array().cloned().unwrap_or_default();
    assert!(
        !lines.is_empty(),
        "the test invocation must appear in host.tool_logs: {logs}"
    );
    assert!(
        lines
            .iter()
            .all(|l| l.as_str().is_some_and(|s| s.starts_with("[test]"))),
        "every line in the spec_test bucket must be marked as a test: {lines:?}"
    );
}

#[tokio::test]
async fn test_invocations_are_distinguishable_from_production_calls() {
    let upstream = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ok": true})))
        .mount(&upstream)
        .await;

    let server = TestServer::start_with_kinds(http_kind_registry()).await;
    let (ns, key) = signup(&server.base_url, "Mixed Caller").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let spec = json!({
        "method": "GET",
        "url": format!("{}/v1/thing", upstream.uri()),
        "args_schema": {"type": "object"},
    });

    // A real, published call...
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "hello", "kind": "http", "spec": spec.clone()}),
        )
        .await
        .expect("publish");
    client
        .tools_call(&format!("{ns}.hello"), json!({}))
        .await
        .expect("production call");

    // ...and a test of the same shape, run as an unpublished spec.
    client
        .tools_call(
            "host.spec_test",
            json!({"kind": "http", "spec": spec, "invocations": [{}]}),
        )
        .await
        .expect("spec_test ok");

    let production_logs = extract_structured(
        &client
            .tools_call("host.tool_logs", json!({"name": "hello"}))
            .await
            .expect("host.tool_logs for the published tool"),
    );
    let production_lines = production_logs["lines"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert!(
        !production_lines.is_empty()
            && production_lines
                .iter()
                .all(|l| l.as_str().is_some_and(|s| !s.starts_with("[test]"))),
        "the published tool's own logs must contain no test-marked lines: {production_lines:?}"
    );

    let test_logs = extract_structured(
        &client
            .tools_call("host.tool_logs", json!({"name": "__spec_test__.http"}))
            .await
            .expect("host.tool_logs for the spec_test bucket"),
    );
    let test_lines = test_logs["lines"].as_array().cloned().unwrap_or_default();
    assert!(
        !test_lines.is_empty()
            && test_lines
                .iter()
                .all(|l| l.as_str().is_some_and(|s| s.starts_with("[test]"))),
        "the spec_test bucket's lines must all be test-marked: {test_lines:?}"
    );
}
