//! PRD-mcphost-webhook-inbox
//! AC4 (P0) — Given an unknown opaque id, When POSTed, Then 404 with an
//! empty body.

use serde_json::json;

#[tokio::test]
async fn unknown_hook_id_is_404_with_an_empty_body() {
    let server = crate::common::TestServer::start().await;

    let http = reqwest::Client::new();
    let resp = http
        .post(format!("{}/hook/does-not-exist", server.base_url))
        .body(json!({"n": 1}).to_string())
        .send()
        .await
        .expect("POST /hook/...");
    assert_eq!(resp.status(), 404);
    let bytes = resp.bytes().await.expect("read body");
    assert!(bytes.is_empty(), "an unknown hook id must answer with an empty body, got {bytes:?}");
}
