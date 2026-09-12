//! AC6 (P0) — Given a 300 KiB body, When posted, Then 413 and no run.

mod common;
use common::{extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn oversized_body_is_413_and_creates_no_run() {
    let server = common::TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "AC6 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "big_hook", "kind": "echo", "spec": {"schema": {"type": "object"}}}),
        )
        .await
        .expect("publish");
    client
        .tools_call(
            "host.trigger.set",
            json!({
                "tool": "big_hook",
                "kind": "event",
                "verify": {"scheme": "none", "allow_unverified": true},
            }),
        )
        .await
        .expect("trigger.set");

    let http = reqwest::Client::new();
    // 300 KiB, over the free plan's 256 KiB `event_body_bytes_max`.
    let body = vec![b'a'; 300 * 1024];
    let resp = http
        .post(format!("{}/hooks/{}/big_hook", server.base_url, ns))
        .body(body)
        .send()
        .await
        .expect("POST /hooks/...");
    assert_eq!(resp.status(), 413);

    let listed = extract_structured(
        &client
            .tools_call("host.runs.list", json!({"trigger": "event"}))
            .await
            .expect("runs.list"),
    );
    let runs = listed["runs"].as_array().expect("runs array");
    assert!(runs.is_empty(), "an oversized body must create no run: {runs:?}");
}
