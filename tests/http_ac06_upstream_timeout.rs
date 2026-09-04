//! AC6 — Given an upstream that sleeps longer than `timeout_s`, When
//! called, Then the tool error is `upstream_timeout` within `timeout_s` +
//! 1s.

mod common;
use common::{McpClient, TestServer, http_kind_registry, signup};
use serde_json::json;
use std::time::{Duration, Instant};
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn slow_upstream_times_out_within_budget() {
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
    let (ns, key) = signup(&server.base_url, "Timeout Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let spec = json!({
        "method": "GET",
        "url": format!("{}/v1/slow", upstream.uri()),
        "args_schema": {"type": "object"},
        "timeout_s": 1,
    });
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "slow", "kind": "http", "spec": spec}),
        )
        .await
        .expect("publish ok");

    let start = Instant::now();
    let err = client
        .tools_call(&format!("{ns}.slow"), json!({}))
        .await
        .expect_err("a request slower than timeout_s must time out");
    let elapsed = start.elapsed();

    assert_eq!(err.error_code.as_deref(), Some("upstream_timeout"));
    assert!(
        elapsed < Duration::from_secs(2),
        "timeout_s=1 must fail within timeout_s + 1s, took {elapsed:?}"
    );
}
