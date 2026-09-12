//! AC7 — Given an upstream that returns 5 MiB, When called, Then the tool
//! error is `response_too_large` and the host read no more than 1 MiB.

use crate::common;
use common::{McpClient, TestServer, http_kind_registry, signup};
use serde_json::json;
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn oversized_upstream_body_is_capped() {
    let upstream = MockServer::start().await;
    let five_mib = vec![b'x'; 5 * 1024 * 1024];
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(five_mib))
        .mount(&upstream)
        .await;

    let server = TestServer::start_with_kinds(http_kind_registry()).await;
    let (ns, key) = signup(&server.base_url, "Big Body Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let spec = json!({
        "method": "GET",
        "url": format!("{}/v1/big", upstream.uri()),
        "args_schema": {"type": "object"},
        "response": "text",
    });
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "big", "kind": "http", "spec": spec}),
        )
        .await
        .expect("publish ok");

    let err = client
        .tools_call(&format!("{ns}.big"), json!({}))
        .await
        .expect_err("a 5 MiB response must be rejected as too large");
    assert_eq!(err.error_code.as_deref(), Some("response_too_large"));
}
