//! PRD-mcphost-result-envelope-contract AC4 — Given `host.tool_test` on a
//! spec declaring `blocking_tool` whose implementation never emits it, When
//! the test runs, Then the report's `envelope.missing` contains
//! `blocking_tool` and the run is not green.

use crate::common;
use common::{McpClient, TestServer, extract_structured, http_kind_registry, signup};
use serde_json::json;
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn tool_test_names_a_declared_field_the_implementation_never_emits() {
    let upstream = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ok": true})))
        .mount(&upstream)
        .await;

    let server = TestServer::start_with_kinds(http_kind_registry()).await;
    let (_ns, key) = signup(&server.base_url, "Envelope AC4 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let spec = json!({
        "method": "GET",
        "url": format!("{}/v1/orchestrate", upstream.uri()),
        "args_schema": {"type": "object"},
        "outputs": ["blocking_tool"],
    });
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "orchestrator", "kind": "http", "spec": spec}),
        )
        .await
        .expect("publish ok");

    let result = client
        .tools_call(
            "host.tool_test",
            json!({"name": "orchestrator", "args": {}}),
        )
        .await
        .expect("tool_test ok");
    let structured = extract_structured(&result);

    let missing = structured["envelope"]["missing"]
        .as_array()
        .expect("envelope.missing must be an array")
        .iter()
        .filter_map(|v| v.as_str())
        .collect::<Vec<_>>();
    assert!(
        missing.contains(&"blocking_tool"),
        "envelope.missing must name blocking_tool: {structured}"
    );
    assert_eq!(
        structured["envelope"]["green"], false,
        "the run must not be reported green when a declared field is missing: {structured}"
    );
    assert_eq!(structured["envelope"]["declared"], json!(["blocking_tool"]));
}

// PRD-mcphost-dry-run-side-effects requirement 4 supersedes the http half
// of this AC's "green" case: `host.tool_test` now short-circuits the http
// call before the upstream ever answers, so `payload` is always absent
// and a declared field is always reported missing for an http-kind spec --
// renamed from `tool_test_reports_green_when_every_declared_field_is_found`
// to say what it now actually proves. The real "every field found" case
// for http still exists, just not through `host.tool_test` any more; the
// python-kind version of this AC (`runenvelope_ac4_no_calls_row_with_payload`,
// via `host.tool_run`'s real-call path) is unaffected.
#[tokio::test]
async fn tool_test_reports_missing_even_when_upstream_would_have_had_the_field() {
    let upstream = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "blocking_tool": "none",
        })))
        .mount(&upstream)
        .await;

    let server = TestServer::start_with_kinds(http_kind_registry()).await;
    let (_ns, key) = signup(&server.base_url, "Envelope AC4 Green Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let spec = json!({
        "method": "GET",
        "url": format!("{}/v1/orchestrate", upstream.uri()),
        "args_schema": {"type": "object"},
        "outputs": ["blocking_tool"],
    });
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "orchestrator_ok", "kind": "http", "spec": spec}),
        )
        .await
        .expect("publish ok");

    let result = client
        .tools_call(
            "host.tool_test",
            json!({"name": "orchestrator_ok", "args": {}}),
        )
        .await
        .expect("tool_test ok");
    let structured = extract_structured(&result);

    assert_eq!(structured["envelope"]["missing"], json!(["blocking_tool"]));
    assert_eq!(structured["envelope"]["green"], false);
    assert_eq!(structured["dry_run_short_circuited"], json!(true));

    let received = upstream.received_requests().await.expect("mock recorded");
    assert_eq!(
        received.len(),
        0,
        "host.tool_test must short-circuit, never perform the real call"
    );
}
