//! AC1 — Given a fake upstream answering 401 and one answering 403, When the
//! http tool is called, Then the error code is `upstream_auth`,
//! `data.upstream_status` is 401/403, and `data.hint` contains
//! `host.secret.set` and `{{ secret.`.

use crate::common;
use common::{McpClient, TestServer, http_kind_registry, signup};
use serde_json::json;
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

async fn call_status(status: u16) -> common::RpcError {
    let upstream = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(status).set_body_string("nope"))
        .mount(&upstream)
        .await;
    let server = TestServer::start_with_kinds(http_kind_registry()).await;
    let (ns, key) = signup(&server.base_url, "Upfam Auth").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    let spec = json!({
        "method": "GET",
        "url": format!("{}/v1/me", upstream.uri()),
        "args_schema": {"type": "object"},
    });
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "me", "kind": "http", "spec": spec}),
        )
        .await
        .expect("publish ok");
    client
        .tools_call(&format!("{ns}.me"), json!({}))
        .await
        .expect_err("a 4xx upstream must surface as a tool error")
}

#[tokio::test]
async fn upstream_401_and_403_are_upstream_auth_with_secret_hint() {
    for status in [401u16, 403] {
        let err = call_status(status).await;
        assert_eq!(err.error_code.as_deref(), Some("upstream_auth"), "{status}");
        assert_eq!(err.data["upstream_status"], status);
        let hint = err.data["hint"].as_str().expect("hint is a string");
        assert!(hint.contains("host.secret.set"), "{hint}");
        assert!(hint.contains("{{ secret."), "{hint}");
    }
}
