//! PRD-mcphost-public-tool-url
//! AC5 (P0) — Given a client IP that made 60 calls in the current minute,
//! When it calls again, Then 429 with a `Retry-After` header is returned
//! and no run is created.

use crate::common;
use common::{TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn sixty_first_call_in_a_minute_is_rate_limited() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "AC5 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "echo", "kind": "echo", "spec": {"schema": {"type": "object"}}}),
        )
        .await
        .expect("publish ok");
    let shared = extract_structured(
        &client
            .tools_call("host.tool_share", json!({"name": "echo", "visibility": "url"}))
            .await
            .expect("tool_share ok"),
    );
    let url = shared["url"].as_str().expect("url field").to_string();

    let http = reqwest::Client::new();
    for i in 0..60 {
        let resp = http.get(&url).send().await.expect("GET");
        assert_eq!(resp.status(), 200, "call {i} of the first 60 must succeed");
    }

    let runs_before = extract_structured(
        &client
            .tools_call("host.runs.list", json!({"trigger": "url", "limit": 200}))
            .await
            .expect("runs.list"),
    )["runs"]
        .as_array()
        .expect("runs array")
        .len();

    let resp = http.get(&url).send().await.expect("GET (61st)");
    assert_eq!(resp.status(), 429, "the 61st call in the minute must be rate limited");
    let retry_after = resp
        .headers()
        .get("Retry-After")
        .expect("Retry-After header present")
        .to_str()
        .expect("Retry-After is a valid header value");
    assert!(!retry_after.is_empty());

    let runs_after = extract_structured(
        &client
            .tools_call("host.runs.list", json!({"trigger": "url", "limit": 200}))
            .await
            .expect("runs.list"),
    )["runs"]
        .as_array()
        .expect("runs array")
        .len();
    assert_eq!(runs_before, runs_after, "the rate-limited call must not create a run");
}
