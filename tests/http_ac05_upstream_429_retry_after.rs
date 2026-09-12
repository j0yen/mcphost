//! AC5 — Given an upstream that returns 429 with `Retry-After: 7`, When
//! called, Then the tool error is `upstream_status` with status 429 and
//! `retry_after_s: 7`.

use crate::common;
use common::{McpClient, TestServer, http_kind_registry, signup};
use serde_json::json;
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn rate_limited_upstream_surfaces_status_and_retry_after() {
    let upstream = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(
            ResponseTemplate::new(429)
                .insert_header("Retry-After", "7")
                .set_body_string("slow down"),
        )
        .mount(&upstream)
        .await;

    let server = TestServer::start_with_kinds(http_kind_registry()).await;
    let (ns, key) = signup(&server.base_url, "429 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let spec = json!({
        "method": "GET",
        "url": format!("{}/v1/thing", upstream.uri()),
        "args_schema": {"type": "object"},
    });
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "flaky", "kind": "http", "spec": spec}),
        )
        .await
        .expect("publish ok");

    let err = client
        .tools_call(&format!("{ns}.flaky"), json!({}))
        .await
        .expect_err("a 429 upstream must surface as a tool error");
    assert_eq!(err.error_code.as_deref(), Some("upstream_status"));
    assert_eq!(err.data["upstream_status"], 429);
    assert_eq!(err.data["retry_after_s"], 7);
}
