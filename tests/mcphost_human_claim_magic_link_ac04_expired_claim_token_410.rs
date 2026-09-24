//! PRD-mcphost-human-claim-magic-link
//! AC4 (P0) — Given an expired claim token (TTL forced to 1 s in test),
//! When `GET /claim/{token}` is requested, Then 410 with "link expired"
//! and no provider send occurs.

use crate::common;
use common::{McpClient, TestServer, extract_structured};
use serde_json::json;
use std::sync::Arc;

fn token_from_claim_url(claim_url: &str) -> &str {
    claim_url.rsplit('/').next().expect("claim_url has a path segment")
}

#[tokio::test]
async fn expired_claim_token_returns_410_link_expired() {
    let fake = Arc::new(mcphost::email::FakeEmailClient::new());
    let server = TestServer::start_with_email(fake.clone()).await;
    let anon = McpClient::new(&server.base_url);

    let raw = anon
        .tools_call("signup", json!({"name": "AC4 Tenant"}))
        .await
        .expect("signup");
    let result = extract_structured(&raw);
    let token = token_from_claim_url(result["claim_url"].as_str().expect("claim_url")).to_string();

    // TTL forced to 1s in test: rather than a real 1-second sleep, force
    // the already-issued token's expiry into the past directly (same
    // "flip an internal knob" convention `expire_handoff_token_for_test`
    // already uses for the sibling handoff-token TTL).
    server
        .state
        .db
        .expire_claim_token_for_test(mcphost::auth::hash_key(&token))
        .await
        .expect("expire_claim_token_for_test");

    let http = reqwest::Client::new();
    let resp = http
        .get(format!("{}/claim/{token}", server.base_url))
        .send()
        .await
        .expect("GET /claim/{token}");
    assert_eq!(resp.status(), reqwest::StatusCode::GONE);
    let body = resp.text().await.expect("body");
    assert!(body.contains("link expired"), "body: {body}");

    assert_eq!(fake.send_count(), 0, "no provider send for an expired token");
}
