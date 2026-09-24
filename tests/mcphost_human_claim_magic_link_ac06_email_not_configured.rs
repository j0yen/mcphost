//! PRD-mcphost-human-claim-magic-link
//! AC6 (P0) — Given `MCPHOST_EMAIL_API_URL` unset, When `GET
//! /claim/{token}` is requested, Then the page renders with "email
//! delivery is not configured" and admin healthz reports
//! `claim_email_configured=false`.

use crate::common;
use common::{ADMIN_KEY, McpClient, TestServer, extract_structured};
use serde_json::json;

fn token_from_claim_url(claim_url: &str) -> &str {
    claim_url.rsplit('/').next().expect("claim_url has a path segment")
}

#[tokio::test]
async fn unconfigured_email_shows_message_and_healthz_flag() {
    // The default TestServer has no email provider configured, same as a
    // real host with $MCPHOST_EMAIL_API_URL unset.
    let server = TestServer::start().await;
    let anon = McpClient::new(&server.base_url);

    let raw = anon
        .tools_call("signup", json!({"name": "AC6 Tenant"}))
        .await
        .expect("signup");
    let result = extract_structured(&raw);
    let token = token_from_claim_url(result["claim_url"].as_str().expect("claim_url"));

    let http = reqwest::Client::new();
    let resp = http
        .get(format!("{}/claim/{token}", server.base_url))
        .send()
        .await
        .expect("GET /claim/{token}");
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    let body = resp.text().await.expect("body");
    assert!(
        body.contains("email delivery is not configured"),
        "body: {body}"
    );

    let healthz: serde_json::Value = http
        .get(format!("{}/healthz", server.base_url))
        .bearer_auth(ADMIN_KEY)
        .send()
        .await
        .expect("GET /healthz")
        .json()
        .await
        .expect("parse /healthz");
    assert_eq!(healthz["claim_email_configured"], json!(false));
}
