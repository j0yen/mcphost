//! PRD-mcphost-send-email-from-tool
//! AC3 (P0) -- Given the agent calls `host.email.send` directly with `html`,
//! When sent through the Resend shape, Then the request body carries `html`
//! and `text` (and Postmark's carries `HtmlBody` and `TextBody`); the claim
//! email's body is the same plain `{from, to, reply_to, subject, text}` it
//! always was, so the claim-email suite is untouched.

use crate::common;
use common::{ADMIN_KEY, McpClient, TestServer, extract_structured, signup};
use mcphost::email::{EmailClient, EmailConfig, EmailMessage, EmailProvider, HttpEmailClient};
use serde_json::{Value, json};
use std::sync::Arc;
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

const OWNER: &str = "owner@example.com";

async fn server_with_provider(mock: &MockServer, provider: EmailProvider) -> TestServer {
    let api_url = mock.uri();
    let email_config = EmailConfig {
        api_url: Some(api_url.clone()),
        api_key: Some("emsnd-ac03-key".to_string()),
        from: Some("claim@mcphost.dev".to_string()),
        provider,
    };
    let email_client: Arc<dyn EmailClient> =
        Arc::new(HttpEmailClient::new(reqwest::Client::new(), api_url, email_config.api_key.clone(), provider));
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

async fn only_request_body(mock: &MockServer) -> Value {
    let requests = mock.received_requests().await.expect("recording is on");
    assert_eq!(requests.len(), 1, "exactly one provider request");
    serde_json::from_slice(&requests[0].body).expect("JSON body")
}

#[tokio::test]
async fn emsnd_ac03_html_and_text_ride_the_resend_body() {
    let mock = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"id": "re_abc123"})))
        .mount(&mock)
        .await;
    let server = server_with_provider(&mock, EmailProvider::Resend).await;
    let (ns, key) = signup(&server.base_url, "emsnd ac03 tenant").await;
    let agent = McpClient::with_bearer(&server.base_url, &key);
    let conn = rusqlite::Connection::open(server.data_dir.0.join("mcphost.db")).expect("open raw db");
    conn.execute(
        "UPDATE tenants SET owner_email = ?1, owner_verified_at = 1700000000 WHERE namespace = ?2",
        rusqlite::params![OWNER, ns],
    )
    .expect("verify owner");
    drop(conn);

    let sent = extract_structured(
        &agent
            .tools_call(
                "host.email.send",
                json!({"to": OWNER, "subject": "Weekly report", "text": "plain body", "html": "<p>rich body</p>"}),
            )
            .await
            .expect("send"),
    );
    assert_eq!(sent["provider_message_id"], json!("re_abc123"), "{sent}");

    let body = only_request_body(&mock).await;
    assert_eq!(body["html"], json!("<p>rich body</p>"), "{body}");
    assert_eq!(body["text"], json!("plain body"), "{body}");
    assert_eq!(body["subject"], json!("Weekly report"), "{body}");
    assert_eq!(body["to"], json!(OWNER), "{body}");
    assert!(body["from"].as_str().is_some_and(|f| f.starts_with("agent@")), "{body}");
}

#[tokio::test]
async fn emsnd_ac03_postmark_shape_and_unchanged_claim_body() {
    let mock = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"MessageID": "pm-1"})))
        .mount(&mock)
        .await;
    let postmark = HttpEmailClient::new(reqwest::Client::new(), mock.uri(), Some("k".to_string()), EmailProvider::Postmark);
    let receipt = postmark
        .send_message(&EmailMessage {
            to: "a@b.co".to_string(),
            from: "x@y.dev".to_string(),
            reply_to: "x@y.dev".to_string(),
            subject: "s".to_string(),
            text_body: "t".to_string(),
            html_body: Some("<b>h</b>".to_string()),
            ..Default::default()
        })
        .await
        .expect("postmark send");
    assert_eq!(receipt.provider_message_id.as_deref(), Some("pm-1"));
    let body = only_request_body(&mock).await;
    assert_eq!(body["HtmlBody"], json!("<b>h</b>"), "{body}");
    assert_eq!(body["TextBody"], json!("t"), "{body}");

    // A claim-shaped message (no html, headers or attachments) keeps the
    // original key set: nothing new leaks into the claim path's wire body.
    let mock = MockServer::start().await;
    Mock::given(method("POST")).respond_with(ResponseTemplate::new(200)).mount(&mock).await;
    let resend = HttpEmailClient::new(reqwest::Client::new(), mock.uri(), None, EmailProvider::Resend);
    resend
        .send(&EmailMessage {
            to: "a@b.co".to_string(),
            from: "claim@mcphost.dev".to_string(),
            reply_to: "claim@mcphost.dev".to_string(),
            subject: "claim".to_string(),
            text_body: "link".to_string(),
            ..Default::default()
        })
        .await
        .expect("claim-shaped send");
    let body = only_request_body(&mock).await;
    let mut keys: Vec<&str> = body.as_object().expect("object").keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(keys, ["from", "reply_to", "subject", "text", "to"], "{body}");
}
