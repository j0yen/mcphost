//! PRD-mcphost-onboarding-dead-ends-zero-b AC1 (P0) -- Given an http-kind tool
//! whose spec references secret `API_TOKEN` and an upstream 403, When the
//! error is built, Then `next_call == {tool: "host.secret.set", args: {name:
//! "API_TOKEN"}}`; without a secret reference, `next_call.tool ==
//! "host.tool_test"`.

use crate::common;
use common::{McpClient, TestServer, http_kind_registry, signup};
use serde_json::{Value, json};
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

async fn call_403(with_secret: bool) -> common::RpcError {
    let upstream = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(403).set_body_string("nope"))
        .mount(&upstream)
        .await;
    let server = TestServer::start_with_kinds(http_kind_registry()).await;
    let (ns, key) = signup(&server.base_url, "Ndeadb Ac1").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    let mut spec = json!({
        "method": "GET",
        "url": format!("{}/v1/me", upstream.uri()),
        "args_schema": {"type": "object"},
    });
    if with_secret {
        client
            .tools_call("host.secret_set", json!({"name": "API_TOKEN", "value": "tok-123"}))
            .await
            .expect("secret set");
        spec["headers"] = json!({"Authorization": "Bearer {{ secret.API_TOKEN }}"});
    }
    client
        .tools_call("host.tool_publish", json!({"name": "me", "kind": "http", "spec": spec}))
        .await
        .expect("publish ok");
    client
        .tools_call(&format!("{ns}.me"), json!({}))
        .await
        .expect_err("a 403 upstream must surface as a tool error")
}

#[tokio::test]
async fn ndeadb_ac01_upstream_403_next_call() {
    let err = call_403(true).await;
    assert_eq!(err.error_code.as_deref(), Some("upstream_auth"));
    assert_eq!(
        err.data["next_call"],
        json!({"tool": "host.secret.set", "args": {"name": "API_TOKEN"}}),
        "{}",
        err.data
    );

    let err = call_403(false).await;
    let next: &Value = &err.data["next_call"];
    assert_eq!(next["tool"], "host.tool_test", "{}", err.data);
}
