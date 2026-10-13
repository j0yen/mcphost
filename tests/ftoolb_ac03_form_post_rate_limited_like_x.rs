//! PRD-mcphost-form-tool-b
//! AC3 (P0) — Given 61 POSTs from one IP in an hour (the `/x/` limit), When
//! the 61st arrives, Then it is rate-limited with the same response `/x/`
//! gives and no run row is created.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn sixty_first_form_post_gets_the_x_rate_limit_response_and_creates_no_run() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Ftoolb AC3").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "ping", "kind": "echo", "spec": {"schema": {"type": "object"}, "ui": {"form": true}}}),
        )
        .await
        .expect("publish");
    let shared = extract_structured(
        &client.tools_call("host.tool_share", json!({"name": "ping", "visibility": "url"})).await.expect("share"),
    );
    let form_url = shared["form_url"].as_str().expect("form_url").to_string();
    let x_url = shared["url"].as_str().expect("url").to_string();

    let http = reqwest::Client::new();
    // 60 POSTs from this address; a stale token is refused (403) but still counts.
    for i in 0..60 {
        let r = http.post(&form_url).form(&[("_csrf", "1.stale")]).send().await.unwrap();
        assert_eq!(r.status(), 403, "post {i} is within the limit");
    }
    let runs_before = extract_structured(&client.tools_call("host.runs.list", json!({"limit": 200})).await.unwrap())["runs"]
        .as_array()
        .unwrap()
        .len();

    let limited = http.post(&form_url).form(&[("_csrf", "1.stale")]).send().await.unwrap();
    assert_eq!(limited.status(), 429);
    assert!(limited.headers().get("Retry-After").is_some());
    let form_body = limited.text().await.unwrap();

    // `/x/` from the same address is now limited with the identical body.
    let x_limited = http.post(&x_url).json(&json!({})).send().await.unwrap();
    assert_eq!(x_limited.status(), 429);
    assert_eq!(x_limited.text().await.unwrap(), form_body, "same response as /x/");

    let runs_after = extract_structured(&client.tools_call("host.runs.list", json!({"limit": 200})).await.unwrap())["runs"]
        .as_array()
        .unwrap()
        .len();
    assert_eq!(runs_before, runs_after, "no run row for the limited POST");
}
