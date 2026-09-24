//! PRD-mcphost-human-claim-magic-link
//! AC5 (P0) — Given `POST /claim/{token}` with an empty or malformed
//! email, When submitted, Then 400-class page with the form re-rendered
//! and zero provider sends.

use crate::common;
use common::{McpClient, TestServer, extract_structured};
use serde_json::json;
use std::sync::Arc;

fn token_from_claim_url(claim_url: &str) -> &str {
    claim_url.rsplit('/').next().expect("claim_url has a path segment")
}

async fn signup_and_get_token(server: &TestServer) -> String {
    let anon = McpClient::new(&server.base_url);
    let raw = anon
        .tools_call("signup", json!({"name": "AC5 Tenant"}))
        .await
        .expect("signup");
    let result = extract_structured(&raw);
    token_from_claim_url(result["claim_url"].as_str().expect("claim_url")).to_string()
}

#[tokio::test]
async fn empty_email_is_rejected_with_400_and_re_rendered_form() {
    let fake = Arc::new(mcphost::email::FakeEmailClient::new());
    let server = TestServer::start_with_email(fake.clone()).await;
    let token = signup_and_get_token(&server).await;

    let http = reqwest::Client::new();
    let resp = http
        .post(format!("{}/claim/{token}", server.base_url))
        .form(&[("email", "")])
        .send()
        .await
        .expect("POST /claim/{token}");
    assert_eq!(resp.status(), reqwest::StatusCode::BAD_REQUEST);
    let body = resp.text().await.expect("body");
    assert!(body.contains("<form"), "form must be re-rendered: {body}");
    assert!(body.contains(&format!("/claim/{token}")), "form must post back to this token: {body}");
    assert_eq!(fake.send_count(), 0);
}

#[tokio::test]
async fn malformed_email_is_rejected_with_400_and_re_rendered_form() {
    let fake = Arc::new(mcphost::email::FakeEmailClient::new());
    let server = TestServer::start_with_email(fake.clone()).await;
    let token = signup_and_get_token(&server).await;

    let http = reqwest::Client::new();
    let resp = http
        .post(format!("{}/claim/{token}", server.base_url))
        .form(&[("email", "not-an-email")])
        .send()
        .await
        .expect("POST /claim/{token}");
    assert_eq!(resp.status(), reqwest::StatusCode::BAD_REQUEST);
    let body = resp.text().await.expect("body");
    assert!(body.contains("<form"), "form must be re-rendered: {body}");
    assert_eq!(fake.send_count(), 0);
}
