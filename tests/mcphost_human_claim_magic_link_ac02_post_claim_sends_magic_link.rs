//! PRD-mcphost-human-claim-magic-link
//! AC2 (P0) — Given a valid unexpired claim token and a fake email
//! provider, When `POST /claim/{token}` is sent with `email=a@b.co`, Then
//! exactly one provider send is recorded containing a verify URL, and the
//! page body contains "check your inbox".

use crate::common;
use common::{McpClient, TestServer, extract_structured};
use serde_json::json;
use std::sync::Arc;

fn token_from_claim_url(claim_url: &str) -> &str {
    claim_url.rsplit('/').next().expect("claim_url has a path segment")
}

#[tokio::test]
async fn post_claim_sends_exactly_one_magic_link() {
    let fake = Arc::new(mcphost::email::FakeEmailClient::new());
    let server = TestServer::start_with_email(fake.clone()).await;
    let client = McpClient::new(&server.base_url);

    let raw = client
        .tools_call("signup", json!({"name": "AC2 Tenant"}))
        .await
        .expect("signup");
    let result = extract_structured(&raw);
    let claim_url = result["claim_url"].as_str().expect("claim_url");
    let token = token_from_claim_url(claim_url);

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

    assert_eq!(fake.send_count(), 1, "exactly one provider send");
    let sends = fake.sends();
    assert_eq!(sends[0].to, "a@b.co");
    assert!(
        sends[0].text_body.contains("/claim/verify/"),
        "send body must contain a verify URL: {}",
        sends[0].text_body
    );
}
