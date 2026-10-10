//! AC2 — Given a fake upstream answering 429 with `Retry-After: 7`, When
//! called, Then the code is `upstream_rate_limited`, `data.retry_after_s` is
//! 7, and the hint names `retry_after_s`.

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
async fn upstream_429_is_rate_limited_with_retry_after_hint() {
    let err = call_with(
        ResponseTemplate::new(429)
            .insert_header("Retry-After", "7")
            .set_body_string("slow down"),
    )
    .await;
    assert_eq!(err.error_code.as_deref(), Some("upstream_rate_limited"));
    assert_eq!(err.data["upstream_status"], 429);
    assert_eq!(err.data["retry_after_s"], 7);
    let hint = err.data["hint"].as_str().expect("hint is a string");
    assert!(hint.contains("retry_after_s"), "{hint}");
}
