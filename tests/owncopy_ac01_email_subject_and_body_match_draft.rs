//! PRD-mcphost-ownership-copy
//! AC1 (P0) — Given a tenant named "my-agent" and a verify link, When the
//! ownership email is rendered, Then its subject is `Your agent set up
//! my-agent on mcphost — is this yours?` and its body equals the approved
//! draft text with the placeholders filled, in plain text.

use crate::common;
use common::{McpClient, TestServer, extract_structured};
use serde_json::json;
use std::sync::Arc;

fn token_from_claim_url(claim_url: &str) -> &str {
    claim_url.rsplit('/').next().expect("claim_url has a path segment")
}

#[tokio::test]
async fn ownership_email_subject_and_body_match_the_approved_draft() {
    let fake = Arc::new(mcphost::email::FakeEmailClient::new());
    let server = TestServer::start_with_email(fake.clone()).await;
    let anon = McpClient::new(&server.base_url);

    let raw = anon
        .tools_call("signup", json!({"name": "my-agent"}))
        .await
        .expect("signup");
    let result = extract_structured(&raw);
    let token = token_from_claim_url(result["claim_url"].as_str().expect("claim_url")).to_string();

    let http = reqwest::Client::new();
    http.post(format!("{}/claim/{token}", server.base_url))
        .form(&[("email", "owner@example.com")])
        .send()
        .await
        .expect("POST /claim/{token}");

    let sends = fake.sends();
    assert_eq!(sends.len(), 1, "exactly one ownership email sent");
    let message = &sends[0];

    assert_eq!(
        message.subject, "Your agent set up my-agent on mcphost — is this yours?",
        "subject must match the approved draft with the placeholder filled"
    );

    let verify_url_start = message
        .text_body
        .find("https://")
        .expect("verify URL present in body");
    let verify_url = message.text_body[verify_url_start..]
        .split_whitespace()
        .next()
        .expect("verify URL token")
        .to_string();
    assert!(verify_url.contains("/claim/verify/"), "verify_url: {verify_url}");

    let expected = format!(
        "Hi,\n\nAn AI agent entered this address at mcphost.dev a moment ago. It has been\nbuilding a small backend there called \"my-agent\" — which may include tools it published, schedules it set, and workflows that it is managing.\n\nIf that agent works for you, this link makes you its owner:\n\n{verify_url}\n\nOwning it means you can see what the agent built, get back in if it loses\nits key, and optionally upgrade to Pro. The link works once and stops working in 30 minutes.\n\nIf this wasn't your agent, do nothing; nothing changes.\n\n— mcphost\nmcphost.dev · a home for things your agent builds"
    );
    assert_eq!(
        message.text_body, expected,
        "body must equal the approved draft text with placeholders filled, in plain text"
    );
}
