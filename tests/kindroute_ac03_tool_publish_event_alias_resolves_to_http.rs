//! PRD-mcphost-unknown-kind-routes-to-recipe
//! AC3 (P0) -- Given `host.tool_publish` with a valid `http` spec body but
//! `kind: "event"`, When the server responds, Then the tool is created
//! with runtime kind `http`, the descriptor carries `resolved_from:
//! "event"`, and `host.tool_call` on it succeeds.

use crate::common;
use common::{McpClient, TestServer, extract_structured, http_kind_registry, signup};
use serde_json::json;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn tool_publish_event_alias_creates_http_tool_and_calls_succeed() {
    let upstream = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/ping"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"pong": true})))
        .mount(&upstream)
        .await;

    let server = TestServer::start_with_kinds(http_kind_registry()).await;
    let (_ns, key) = signup(&server.base_url, "Kindroute AC3 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let spec = json!({"method": "GET", "url": format!("{}/ping", upstream.uri())});
    let published = extract_structured(
        &client
            .tools_call(
                "host.tool_publish",
                json!({"name": "on_event", "kind": "event", "spec": spec}),
            )
            .await
            .expect("publish ok"),
    );

    assert_eq!(published["kind"], json!("http"));
    assert_eq!(published["resolved_from"], json!("event"));

    let result = client
        .tools_call("host.tool_call", json!({"name": "on_event", "args": {}}))
        .await
        .expect("host.tool_call succeeds on the published tool");
    let structured = extract_structured(&result);
    assert_eq!(structured["status"], json!(200));
    assert_eq!(structured["body"]["pong"], json!(true));
}
