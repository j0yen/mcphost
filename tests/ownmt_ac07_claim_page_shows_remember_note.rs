//! PRD-mcphost-ownership-moment
//! AC7 (P1) — Given a tenant with a first-call `remember` note, When its
//! claim page renders, Then the note is shown with the tool list.

use crate::common;
use common::{McpClient, TestServer, extract_structured};
use serde_json::json;
use std::sync::Arc;

fn token_from_claim_url(claim_url: &str) -> &str {
    claim_url.rsplit('/').next().expect("claim_url has a path segment")
}

fn code_from_email_body(body: &str) -> &str {
    let idx = body.find("/claim/verify/").expect("verify URL in email body");
    let after = &body[idx + "/claim/verify/".len()..];
    after.split_whitespace().next().expect("code token")
}

#[tokio::test]
async fn claim_summary_page_shows_the_remember_note_with_the_tool_list() {
    let fake = Arc::new(mcphost::email::FakeEmailClient::new());
    let server = TestServer::start_with_email(fake.clone()).await;
    let anon = McpClient::new(&server.base_url);

    let raw = anon
        .tools_call(
            "signup",
            json!({"name": "AC7 Tenant", "remember": "Building a recipe bot for my sister"}),
        )
        .await
        .expect("signup");
    let result = extract_structured(&raw);
    let claim_token = token_from_claim_url(result["claim_url"].as_str().expect("claim_url"));
    let key = result["key"].as_str().expect("key").to_string();

    let tenant_client = McpClient::with_bearer(&server.base_url, &key);
    tenant_client
        .tools_call(
            "host.tool_publish",
            json!({"name": "recipe_lookup", "kind": "echo", "spec": {"schema": {"type": "object"}}}),
        )
        .await
        .expect("publish");

    let http = reqwest::Client::new();
    http.post(format!("{}/claim/{claim_token}", server.base_url))
        .form(&[("email", "a@b.co")])
        .send()
        .await
        .expect("POST /claim/{token}");
    let sends = fake.sends();
    let code = code_from_email_body(&sends[0].text_body).to_string();

    let verify_resp = http
        .get(format!("{}/claim/verify/{code}", server.base_url))
        .send()
        .await
        .expect("GET /claim/verify/{code}");
    assert_eq!(verify_resp.status(), reqwest::StatusCode::OK);
    let body = verify_resp.text().await.expect("body");

    assert!(
        body.contains("Building a recipe bot for my sister"),
        "summary page must show the agent's remember note: {body}"
    );
    assert!(body.contains("recipe_lookup"), "summary must still list the tool name: {body}");
}

#[tokio::test]
async fn claim_summary_page_omits_the_note_block_when_none_was_ever_sent() {
    let fake = Arc::new(mcphost::email::FakeEmailClient::new());
    let server = TestServer::start_with_email(fake.clone()).await;
    let anon = McpClient::new(&server.base_url);

    let raw = anon
        .tools_call("signup", json!({"name": "AC7 No Note Tenant"}))
        .await
        .expect("signup");
    let result = extract_structured(&raw);
    let claim_token = token_from_claim_url(result["claim_url"].as_str().expect("claim_url"));

    let http = reqwest::Client::new();
    http.post(format!("{}/claim/{claim_token}", server.base_url))
        .form(&[("email", "b@b.co")])
        .send()
        .await
        .expect("POST /claim/{token}");
    let sends = fake.sends();
    let code = code_from_email_body(&sends[0].text_body).to_string();

    let verify_resp = http
        .get(format!("{}/claim/verify/{code}", server.base_url))
        .send()
        .await
        .expect("GET /claim/verify/{code}");
    assert_eq!(verify_resp.status(), reqwest::StatusCode::OK);
    let body = verify_resp.text().await.expect("body");
    assert!(
        !body.contains("Your agent said"),
        "a tenant with no remember note must show no note block: {body}"
    );
}
