//! AC13 (P1) — Given a tenant exceeding 600 calls in a minute, When it
//! calls again, Then `rate_limited` with `retry_after_s` is returned and
//! the upstream is not called.
//!
//! Drives the limiter through the real dispatch path (`host.tool_publish`
//! and repeated `tools/call`), but with the per-kind rate limit overridden
//! to a small number (`http_kind_registry_with_rate_limit`) -- proving 600
//! real HTTP round trips land in the same few hundred milliseconds a unit
//! test would take is not what this AC is about; wiring the real limiter
//! into the real call path is.

use crate::common;
use common::{McpClient, TestServer, http_kind_registry_with_rate_limit, signup};
use serde_json::json;
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn exceeding_the_limit_refuses_without_calling_upstream() {
    let upstream = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ok": true})))
        .mount(&upstream)
        .await;

    let server = TestServer::start_with_kinds(http_kind_registry_with_rate_limit(3)).await;
    let (ns, key) = signup(&server.base_url, "Rate Limited Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let spec = json!({
        "method": "GET",
        "url": format!("{}/v1/thing", upstream.uri()),
        "args_schema": {"type": "object"},
    });
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "limited", "kind": "http", "spec": spec}),
        )
        .await
        .expect("publish ok");

    for i in 0..3 {
        client
            .tools_call(&format!("{ns}.limited"), json!({}))
            .await
            .unwrap_or_else(|e| {
                panic!(
                    "call {i} within the limit must succeed: {} {}",
                    e.code, e.message
                )
            });
    }

    let err = client
        .tools_call(&format!("{ns}.limited"), json!({}))
        .await
        .expect_err("the 4th call within the window must be rate limited");
    assert_eq!(err.error_code.as_deref(), Some("rate_limited"));
    assert!(err.data["retry_after_s"].as_u64().is_some_and(|s| s > 0));

    let requests = upstream
        .received_requests()
        .await
        .expect("mock server tracks requests");
    assert_eq!(
        requests.len(),
        3,
        "the rate-limited call must not reach the upstream"
    );
}
