//! AC3 — Given fake upstreams answering 404 and 503, When called, Then the
//! codes are `upstream_client_error` and `upstream_error`, and the 503 hint
//! names `budget.max_tool_latency_ms`.

use crate::common;
use common::{McpClient, TestServer, http_kind_registry, signup};
use serde_json::json;
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

async fn call_with(template: ResponseTemplate) -> common::RpcError {
    let upstream = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(template)
        .mount(&upstream)
        .await;
    let server = TestServer::start_with_kinds(http_kind_registry()).await;
    let (ns, key) = signup(&server.base_url, "Upfam Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    let spec = json!({
        "method": "GET",
        "url": format!("{}/v1/thing", upstream.uri()),
        "args_schema": {"type": "object"},
    });
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "thing", "kind": "http", "spec": spec}),
        )
        .await
        .expect("publish ok");
    client
        .tools_call(&format!("{ns}.thing"), json!({}))
        .await
        .expect_err("a 4xx/5xx upstream must surface as a tool error")
}

#[tokio::test]
async fn upstream_404_is_client_error_and_503_is_error_with_budget_hint() {
    let err404 = call_with(ResponseTemplate::new(404).set_body_string("missing")).await;
    assert_eq!(err404.error_code.as_deref(), Some("upstream_client_error"));
    assert_eq!(err404.data["upstream_status"], 404);
    assert!(err404.data["hint"].as_str().is_some_and(|h| !h.is_empty()));

    let err503 = call_with(ResponseTemplate::new(503).set_body_string("down")).await;
    assert_eq!(err503.error_code.as_deref(), Some("upstream_error"));
    assert_eq!(err503.data["upstream_status"], 503);
    let hint = err503.data["hint"].as_str().expect("hint is a string");
    assert!(hint.contains("budget.max_tool_latency_ms"), "{hint}");
}
