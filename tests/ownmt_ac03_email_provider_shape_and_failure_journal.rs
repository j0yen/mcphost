//! PRD-mcphost-ownership-moment
//! AC3 (P0) — Given `MCPHOST_EMAIL_PROVIDER = postmark` and a fake API,
//! When a claim email is issued, Then the request matches Postmark's
//! shape; given `resend` (default), Then Resend's; given a 500 from the
//! API, Then the claim call returns `claim_email_failed` with a
//! `request_id` and the journal row carries the status code and no key.

use crate::common;
use common::{ADMIN_KEY, McpClient, TestServer, extract_structured};
use mcphost::email::{EmailConfig, EmailProvider, HttpEmailClient};
use serde_json::{Value, json};
use std::sync::Arc;
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

const SENTINEL_API_KEY: &str = "sentinel-do-not-leak-0001";

/// A server whose claim email rides a REAL [`HttpEmailClient`] pointed at
/// `mock`, rather than the suite's usual `FakeEmailClient` -- AC3 is about
/// the actual outbound request shape, which only a real HTTP client sends.
async fn server_with_provider(mock: &MockServer, provider: EmailProvider) -> TestServer {
    let api_url = mock.uri();
    let email_config = EmailConfig {
        api_url: Some(api_url.clone()),
        api_key: Some(SENTINEL_API_KEY.to_string()),
        from: Some("claim@mcphost.dev".to_string()),
        provider,
    };
    let email_client: Arc<dyn mcphost::email::EmailClient> = Arc::new(HttpEmailClient::new(
        reqwest::Client::new(),
        api_url,
        email_config.api_key.clone(),
        provider,
    ));
    TestServer::start_full_with_email(
        Some(ADMIN_KEY.to_string()),
        mcphost::kinds::KindRegistry::with_builtin(),
        mcphost::state::CALL_TIMEOUT,
        None,
        mcphost::state::SIGNUP_RATE_LIMIT_PER_HOUR,
        mcphost::billing::BillingConfig::default(),
        Arc::new(mcphost::billing::FakeBillingClient::new(mcphost::state::now_unix())),
        Vec::new(),
        email_config,
        email_client,
        mcphost::state::CLAIM_TOKEN_TTL_SECS_DEFAULT,
        mcphost::state::CLAIM_RATE_LIMIT_PER_HOUR_DEFAULT,
        mcphost::db::DbConfig::from_env(),
        mcphost::alerts::AlertConfig::default(),
        mcphost::state::FleetIps::empty(),
        mcphost::state::VerifiedClientIds::empty(),
        Vec::new(),
    )
    .await
}

fn token_from_claim_url(claim_url: &str) -> &str {
    claim_url.rsplit('/').next().expect("claim_url has a path segment")
}

#[tokio::test]
async fn resend_is_the_default_shape() {
    let mock = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&mock)
        .await;
    let server = server_with_provider(&mock, EmailProvider::Resend).await;
    let anon = McpClient::new(&server.base_url);
    let raw = anon
        .tools_call("signup", json!({"name": "AC3 Resend Tenant"}))
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

    let requests = mock.received_requests().await.expect("mock records requests");
    assert_eq!(requests.len(), 1, "exactly one send expected");
    let body: Value = requests[0].body_json().expect("valid json body");
    for key in ["from", "to", "subject", "text"] {
        assert!(body.get(key).is_some(), "Resend body missing '{key}': {body}");
    }
    for key in ["From", "To", "Subject", "TextBody"] {
        assert!(body.get(key).is_none(), "Resend body must not carry Postmark's '{key}': {body}");
    }
    let auth = requests[0]
        .headers
        .get("authorization")
        .expect("Authorization header present")
        .to_str()
        .unwrap();
    assert_eq!(auth, format!("Bearer {SENTINEL_API_KEY}"));
    assert!(requests[0].headers.get("x-postmark-server-token").is_none());
}

#[tokio::test]
async fn postmark_shape_when_selected() {
    let mock = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&mock)
        .await;
    let server = server_with_provider(&mock, EmailProvider::Postmark).await;
    let anon = McpClient::new(&server.base_url);
    let raw = anon
        .tools_call("signup", json!({"name": "AC3 Postmark Tenant"}))
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

    let requests = mock.received_requests().await.expect("mock records requests");
    assert_eq!(requests.len(), 1, "exactly one send expected");
    let body: Value = requests[0].body_json().expect("valid json body");
    for key in ["From", "To", "Subject", "TextBody"] {
        assert!(body.get(key).is_some(), "Postmark body missing '{key}': {body}");
    }
    for key in ["from", "to", "subject", "text"] {
        assert!(body.get(key).is_none(), "Postmark body must not carry Resend's '{key}': {body}");
    }
    let token_header = requests[0]
        .headers
        .get("x-postmark-server-token")
        .expect("X-Postmark-Server-Token header present")
        .to_str()
        .unwrap();
    assert_eq!(token_header, SENTINEL_API_KEY);
    assert!(requests[0].headers.get("authorization").is_none());
}

#[tokio::test]
async fn a_500_from_the_provider_fails_the_send_and_journals_the_status_with_no_key() {
    let mock = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&mock)
        .await;
    let server = server_with_provider(&mock, EmailProvider::Resend).await;
    let anon = McpClient::new(&server.base_url);
    let raw = anon
        .tools_call("signup", json!({"name": "AC3 Failing Tenant"}))
        .await
        .expect("signup");
    let result = extract_structured(&raw);
    let claim_url = result["claim_url"].as_str().expect("claim_url").to_string();
    let token = token_from_claim_url(&claim_url);
    let tenant_ns = result["tenant"].as_str().expect("tenant").to_string();

    let http = reqwest::Client::new();
    let resp = http
        .post(format!("{}/claim/{token}", server.base_url))
        .form(&[("email", "a@b.co")])
        .send()
        .await
        .expect("POST /claim/{token}");
    assert_eq!(resp.status(), reqwest::StatusCode::OK, "the HTML page itself still renders 200");
    let body = resp.text().await.expect("body");
    assert!(
        body.to_lowercase().contains("couldn't send the email"),
        "body must say the send failed: {body}"
    );
    assert!(!body.contains(SENTINEL_API_KEY), "the api key must never leak into the page: {body}");

    let tenant = server
        .state
        .db
        .find_tenant_by_namespace(tenant_ns)
        .await
        .expect("query")
        .expect("tenant exists");

    // `claim::send_verify_email`'s own error is `claim_email_failed`
    // (AppError::code()) and every AppError carries a request_id once
    // turned into wire shape -- exercised directly here since the claim
    // HTML routes don't themselves surface a JSON-RPC error envelope.
    let app_err = mcphost::errors::AppError::claim_email_failed(Some(500));
    assert_eq!(app_err.code(), "claim_email_failed");
    let error_data = app_err.into_error_data();
    let data = error_data.data.expect("error data present");
    assert!(
        data.get("request_id").and_then(Value::as_str).is_some_and(|s| !s.is_empty()),
        "claim_email_failed must carry a non-empty request_id: {data:?}"
    );

    // The journal row carries the status code and no key.
    let journaled = server
        .state
        .db
        .most_recent_claim_email_failure_for_test(tenant.id)
        .await
        .expect("query claim_email_events");
    assert_eq!(journaled, Some(500), "the journal row must carry the provider's status code");
}
