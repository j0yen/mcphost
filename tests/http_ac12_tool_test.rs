//! AC12 (P1) — Given `host.tool_test`, When called, Then the response
//! includes the rendered request with secrets replaced by `***` and no
//! `calls` row is written.

mod common;
use common::{McpClient, TestServer, extract_structured, http_kind_registry, signup};
use serde_json::json;
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn tool_test_echoes_redacted_request_and_skips_the_calls_row() {
    let upstream = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ok": true})))
        .mount(&upstream)
        .await;

    let server = TestServer::start_with_kinds(http_kind_registry()).await;
    let (_ns, key) = signup(&server.base_url, "Tool Test Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call(
            "host.secret_set",
            json!({"name": "api_key", "value": "topsecretvalue"}),
        )
        .await
        .expect("secret_set ok");

    let spec = json!({
        "method": "GET",
        "url": format!("{}/v1/thing", upstream.uri()),
        "headers": {"X-Api-Key": "{{secret.api_key}}"},
        "args_schema": {"type": "object"},
    });
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "testable", "kind": "http", "spec": spec}),
        )
        .await
        .expect("publish ok");

    // Before: `host.usage` shows zero calls.
    let usage_before = client
        .tools_call("host.usage", json!({}))
        .await
        .expect("usage ok");
    assert_eq!(extract_structured(&usage_before)["calls"], 0);

    let result = client
        .tools_call("host.tool_test", json!({"name": "testable", "args": {}}))
        .await
        .expect("tool_test ok");
    let structured = extract_structured(&result);

    assert_eq!(structured["response"]["status"], 200);
    let request_dump = structured["request"].to_string();
    assert!(
        !request_dump.contains("topsecretvalue"),
        "tool_test must redact secrets from the echoed request: {request_dump}"
    );
    assert!(
        request_dump.contains("***"),
        "expected the redacted secret marker in the echoed request: {request_dump}"
    );

    // After: still zero -- `host.tool_test` must not have written a `calls`
    // row, even though it really hit the upstream.
    let usage_after = client
        .tools_call("host.usage", json!({}))
        .await
        .expect("usage ok");
    assert_eq!(
        extract_structured(&usage_after)["calls"],
        0,
        "host.tool_test must not record a `calls` row"
    );

    let requests = upstream
        .received_requests()
        .await
        .expect("mock server tracks requests");
    assert_eq!(
        requests.len(),
        1,
        "host.tool_test must perform the real call"
    );
}
