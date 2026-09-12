//! PRD-mcphost-rest-bridge AC4 — Given upstream timeout, 500-with-body, and
//! connection-refused cases on the mock, When the bridge tool (published
//! via a declarative `upstream` spec) is called, Then each returns a
//! structured tool error naming the failure class and excerpting the body
//! where one exists. This is the same error-mapping the hand-templated
//! `http` kind already has (`classify_reqwest_error` /
//! `upstream_status` handling in `kinds::http`) -- the bridge reuses it
//! rather than reimplementing it, since a compiled `upstream` spec runs
//! through the exact same `call()` path.

use crate::common;
use common::{McpClient, TestServer, http_kind_registry, signup};
use serde_json::json;
use std::time::Duration;
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

fn free_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind ephemeral port");
    listener.local_addr().unwrap().port()
}

fn upstream_spec(url: String, timeout_s: Option<u64>) -> serde_json::Value {
    let mut spec = json!({
        "upstream": {
            "url": url,
            "method": "GET",
            "params": {},
        },
    });
    if let Some(t) = timeout_s {
        spec["timeout_s"] = json!(t);
    }
    spec
}

#[tokio::test]
async fn upstream_timeout_is_reported_as_upstream_timeout() {
    let upstream = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"ok": true}))
                .set_delay(Duration::from_secs(5)),
        )
        .mount(&upstream)
        .await;

    let server = TestServer::start_with_kinds(http_kind_registry()).await;
    let (ns, key) = signup(&server.base_url, "Bridge Timeout Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let spec = upstream_spec(format!("{}/v1/slow", upstream.uri()), Some(1));
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "slow_bridge", "kind": "http", "spec": spec}),
        )
        .await
        .expect("publish ok");

    let err = client
        .tools_call(&format!("{ns}.slow_bridge"), json!({}))
        .await
        .expect_err("a request slower than timeout_s must time out");
    assert_eq!(err.error_code.as_deref(), Some("upstream_timeout"));
}

#[tokio::test]
async fn upstream_500_with_body_excerpts_the_body() {
    let upstream = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(500).set_body_string("internal explosion"))
        .mount(&upstream)
        .await;

    let server = TestServer::start_with_kinds(http_kind_registry()).await;
    let (ns, key) = signup(&server.base_url, "Bridge 500 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let spec = upstream_spec(format!("{}/v1/broken", upstream.uri()), None);
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "broken_bridge", "kind": "http", "spec": spec}),
        )
        .await
        .expect("publish ok");

    let err = client
        .tools_call(&format!("{ns}.broken_bridge"), json!({}))
        .await
        .expect_err("a 500 upstream must surface as a tool error");
    assert_eq!(err.error_code.as_deref(), Some("upstream_status"));
    assert_eq!(err.data["upstream_status"], 500);
    assert!(
        err.data["body_excerpt"]
            .as_str()
            .unwrap_or_default()
            .contains("internal explosion"),
        "expected the response body excerpted into the error: {:?}",
        err.data
    );
}

#[tokio::test]
async fn connection_refused_is_reported_as_upstream_unreachable() {
    let port = free_port();
    // Nothing is listening on this loopback port -- the connection attempt
    // itself must fail (ECONNREFUSED), not hang or time out.
    let dead_url = format!("http://127.0.0.1:{port}/v1/nothing");

    let server = TestServer::start_with_kinds(http_kind_registry()).await;
    let (ns, key) = signup(&server.base_url, "Bridge Refused Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let spec = upstream_spec(dead_url, None);
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "dead_bridge", "kind": "http", "spec": spec}),
        )
        .await
        .expect("publish ok");

    let err = client
        .tools_call(&format!("{ns}.dead_bridge"), json!({}))
        .await
        .expect_err("a refused connection must surface as a tool error");
    assert_eq!(err.error_code.as_deref(), Some("upstream_unreachable"));
}
