//! PRD-mcphost-human-claim-magic-link
//! AC10 (P0) — Given the fake email provider returns 500 on the first
//! attempt and 200 on the retry, When the magic link is requested, Then
//! one send is ultimately recorded and the page shows "check your inbox".

use crate::common;
use common::{McpClient, TestServer, extract_structured};
use serde_json::json;
use std::sync::Arc;

fn token_from_claim_url(claim_url: &str) -> &str {
    claim_url.rsplit('/').next().expect("claim_url has a path segment")
}

#[tokio::test]
async fn one_provider_failure_recovers_on_retry() {
    let fake = Arc::new(mcphost::email::FakeEmailClient::new());
    fake.fail_next(1);
    let server = TestServer::start_with_email(fake.clone()).await;
    let anon = McpClient::new(&server.base_url);

    let raw = anon
        .tools_call("signup", json!({"name": "AC10 Tenant"}))
        .await
        .expect("signup");
    let result = extract_structured(&raw);
    let token = token_from_claim_url(result["claim_url"].as_str().expect("claim_url"));

    let http = reqwest::Client::new();
    let resp = http
        .post(format!("{}/claim/{token}", server.base_url))
        .form(&[("email", "a@b.co")])
        .send()
        .await
        .expect("POST /claim/{token}");
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    let body = resp.text().await.expect("body");
    assert!(body.contains("check your inbox"), "body: {body}");

    assert_eq!(fake.send_count(), 1, "exactly one send recorded after the retry");
}
