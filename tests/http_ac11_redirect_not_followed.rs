//! AC11 — Given an upstream that redirects to another host, When called,
//! Then the redirect is not followed and the result reports the 3xx
//! status.

use crate::common;
use common::{McpClient, TestServer, extract_structured, http_kind_registry, signup};
use serde_json::json;
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn redirect_is_reported_not_followed() {
    let upstream = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(
            ResponseTemplate::new(302).insert_header("Location", "https://attacker.example/steal"),
        )
        .mount(&upstream)
        .await;

    let server = TestServer::start_with_kinds(http_kind_registry()).await;
    let (ns, key) = signup(&server.base_url, "Redirect Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let spec = json!({
        "method": "GET",
        "url": format!("{}/v1/thing", upstream.uri()),
        "args_schema": {"type": "object"},
    });
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "redirecting", "kind": "http", "spec": spec}),
        )
        .await
        .expect("publish ok");

    // A 3xx is not an `upstream_status` error (that's reserved for 4xx/5xx)
    // -- it's a normal, successful result reporting the status as-is.
    let result = client
        .tools_call(&format!("{ns}.redirecting"), json!({}))
        .await
        .expect("a 3xx response must not be treated as a tool error");
    let structured = extract_structured(&result);
    assert_eq!(structured["status"], 302);

    // Only the one request the client itself made -- no follow-up hop to
    // `attacker.example` (which wiremock wouldn't have a mock for anyway,
    // and which would fail SSRF validation regardless).
    let requests = upstream
        .received_requests()
        .await
        .expect("mock server tracks requests");
    assert_eq!(requests.len(), 1, "the redirect must not be followed");
}
