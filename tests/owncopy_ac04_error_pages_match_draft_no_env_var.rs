//! PRD-mcphost-ownership-copy
//! AC4 (P0) — Given each of the seven error cases, When its page is
//! rendered, Then the title and body are the draft's and no
//! environment-variable name appears on the page.

use crate::common;
use common::{ADMIN_KEY, McpClient, TestServer, extract_structured};
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

fn assert_no_env_var_name(label: &str, body: &str) {
    assert!(
        !body.contains("MCPHOST_"),
        "{label} must never name an env var on the page: {body}"
    );
}

#[tokio::test]
async fn expired_claim_token_shows_the_drafts_expired_page() {
    let fake = Arc::new(mcphost::email::FakeEmailClient::new());
    let server = TestServer::start_with_email(fake.clone()).await;
    let anon = McpClient::new(&server.base_url);

    let raw = anon
        .tools_call("signup", json!({"name": "AC4 Expired Tenant"}))
        .await
        .expect("signup");
    let result = extract_structured(&raw);
    let token = token_from_claim_url(result["claim_url"].as_str().expect("claim_url")).to_string();

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
    assert!(body.contains("That link has expired"), "body: {body}");
    assert!(
        body.contains("Links last 30 minutes. Ask your agent for a new one, or go back to the page it gave you and request another."),
        "body: {body}"
    );
    assert_no_env_var_name("expired", &body);
}

#[tokio::test]
async fn already_used_verify_code_shows_the_drafts_already_used_page() {
    let fake = Arc::new(mcphost::email::FakeEmailClient::new());
    let server = TestServer::start_with_email(fake.clone()).await;
    let anon = McpClient::new(&server.base_url);

    let raw = anon
        .tools_call("signup", json!({"name": "AC4 AlreadyUsed Tenant"}))
        .await
        .expect("signup");
    let result = extract_structured(&raw);
    let token = token_from_claim_url(result["claim_url"].as_str().expect("claim_url")).to_string();

    let http = reqwest::Client::new();
    http.post(format!("{}/claim/{token}", server.base_url))
        .form(&[("email", "a@b.co")])
        .send()
        .await
        .expect("POST /claim/{token}");
    let code = code_from_email_body(&fake.sends()[0].text_body).to_string();

    http.get(format!("{}/claim/verify/{code}", server.base_url))
        .send()
        .await
        .expect("first GET /claim/verify/{code}");
    let resp = http
        .get(format!("{}/claim/verify/{code}", server.base_url))
        .send()
        .await
        .expect("second GET /claim/verify/{code}");
    assert_eq!(resp.status(), reqwest::StatusCode::GONE);
    let body = resp.text().await.expect("body");
    assert!(body.contains("This link was already used"), "body: {body}");
    assert!(
        body.contains("If you finished on an earlier visit, you're done. If not, ask your agent for a fresh link."),
        "body: {body}"
    );
    assert_no_env_var_name("already used", &body);
}

#[tokio::test]
async fn racing_verify_loser_shows_the_drafts_someone_else_first_page() {
    let fake = Arc::new(mcphost::email::FakeEmailClient::new());
    let server = TestServer::start_with_email(fake.clone()).await;
    let anon = McpClient::new(&server.base_url);

    let raw = anon
        .tools_call("signup", json!({"name": "AC4 Conflict Tenant"}))
        .await
        .expect("signup");
    let result = extract_structured(&raw);
    let token = token_from_claim_url(result["claim_url"].as_str().expect("claim_url")).to_string();

    let http = reqwest::Client::new();
    http.post(format!("{}/claim/{token}", server.base_url))
        .form(&[("email", "first@example.com")])
        .send()
        .await
        .expect("first POST /claim/{token}");
    http.post(format!("{}/claim/{token}", server.base_url))
        .form(&[("email", "second@example.com")])
        .send()
        .await
        .expect("second POST /claim/{token}");
    let sends = fake.sends();
    assert_eq!(sends.len(), 2);
    let code_a = code_from_email_body(&sends[0].text_body).to_string();
    let code_b = code_from_email_body(&sends[1].text_body).to_string();

    let base = server.base_url.clone();
    let (resp_a, resp_b) = tokio::join!(
        http.get(format!("{base}/claim/verify/{code_a}")).send(),
        http.get(format!("{base}/claim/verify/{code_b}")).send(),
    );
    let resp_a = resp_a.expect("verify a");
    let resp_b = resp_b.expect("verify b");
    let loser = if resp_a.status() == reqwest::StatusCode::CONFLICT {
        resp_a
    } else {
        resp_b
    };
    assert_eq!(loser.status(), reqwest::StatusCode::CONFLICT);
    let body = loser.text().await.expect("body");
    assert!(body.contains("Someone else finished first"), "body: {body}");
    assert!(
        body.contains("Another address became the owner of this backend before this link was opened. If that wasn't you, write to hello@mcphost.dev."),
        "body: {body}"
    );
    assert_no_env_var_name("someone else first", &body);
}

#[tokio::test]
async fn mail_not_configured_shows_the_drafts_page_with_no_env_var_name() {
    let server = TestServer::start().await;
    let anon = McpClient::new(&server.base_url);

    let raw = anon
        .tools_call("signup", json!({"name": "AC4 Unconfigured Tenant"}))
        .await
        .expect("signup");
    let result = extract_structured(&raw);
    let token = token_from_claim_url(result["claim_url"].as_str().expect("claim_url")).to_string();

    let http = reqwest::Client::new();
    let resp = http
        .get(format!("{}/claim/{token}", server.base_url))
        .send()
        .await
        .expect("GET /claim/{token}");
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    let body = resp.text().await.expect("body");
    assert!(body.contains("We can't send email from this host yet"), "body: {body}");
    assert!(
        body.contains("The operator hasn't set up outgoing mail. Try again later."),
        "body: {body}"
    );
    assert_no_env_var_name("mail not configured", &body);
}

#[tokio::test]
async fn rate_limited_shows_the_drafts_too_many_tries_page() {
    let server = TestServer::start().await;
    let anon = McpClient::new(&server.base_url);

    let raw = anon
        .tools_call("signup", json!({"name": "AC4 RateLimit Tenant"}))
        .await
        .expect("signup");
    let result = extract_structured(&raw);
    let token = token_from_claim_url(result["claim_url"].as_str().expect("claim_url")).to_string();

    let http = reqwest::Client::new();
    let mut last_body = String::new();
    let mut last_status = reqwest::StatusCode::OK;
    for _ in 0..31 {
        let resp = http
            .get(format!("{}/claim/{token}", server.base_url))
            .send()
            .await
            .expect("GET /claim/{token}");
        last_status = resp.status();
        last_body = resp.text().await.expect("body");
    }
    assert_eq!(last_status, reqwest::StatusCode::TOO_MANY_REQUESTS);
    assert!(last_body.contains("Too many tries"), "body: {last_body}");
    assert!(
        last_body.contains("Wait a few minutes and try again."),
        "body: {last_body}"
    );
    assert_no_env_var_name("rate limited", &last_body);
}

#[tokio::test]
async fn banned_address_shows_the_drafts_cant_be_used_here_page() {
    let server = TestServer::start().await;
    let addr = "203.0.113.77";
    let since = mcphost::state::now_unix() - 600;
    for _ in 0..30 {
        server
            .state
            .db
            .try_admit_claim_request(addr.to_string(), since, 1000)
            .await
            .expect("try_admit_claim_request");
    }
    mcphost::bans::tick_once(&server.state).await.expect("tick_once");

    let admin = McpClient::with_bearer(&server.base_url, ADMIN_KEY);
    let list = admin
        .tools_call("admin.ban.list", json!({"active_only": true, "subject_kind": "addr"}))
        .await
        .expect("admin.ban.list");
    let rows = extract_structured(&list)["bans"].as_array().cloned().unwrap_or_default();
    rows.iter()
        .find(|r| r["subject"] == json!(addr) && r["auto"] == json!(true))
        .unwrap_or_else(|| panic!("no auto-ban for {addr} in {rows:?}"));

    let http = reqwest::Client::new();
    let resp = http
        .get(format!("{}/claim/verify/nonexistent-code", server.base_url))
        .header("x-forwarded-for", addr)
        .send()
        .await
        .expect("GET /claim/verify/{code}");
    assert_eq!(resp.status(), reqwest::StatusCode::FORBIDDEN);
    let body = resp.text().await.expect("body");
    assert!(body.contains("This address can't be used here"), "body: {body}");
    assert!(
        body.contains("Write to hello@mcphost.dev if you think that's a mistake."),
        "body: {body}"
    );
    assert_no_env_var_name("banned", &body);
}

#[tokio::test]
async fn send_failure_shows_the_drafts_couldnt_send_page() {
    let fake = Arc::new(mcphost::email::FakeEmailClient::new());
    fake.fail_next(2);
    let server = TestServer::start_with_email(fake.clone()).await;
    let anon = McpClient::new(&server.base_url);

    let raw = anon
        .tools_call("signup", json!({"name": "AC4 SendFailed Tenant"}))
        .await
        .expect("signup");
    let result = extract_structured(&raw);
    let token = token_from_claim_url(result["claim_url"].as_str().expect("claim_url")).to_string();

    let http = reqwest::Client::new();
    let resp = http
        .post(format!("{}/claim/{token}", server.base_url))
        .form(&[("email", "a@b.co")])
        .send()
        .await
        .expect("POST /claim/{token}");
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    let body = resp.text().await.expect("body");
    assert!(body.contains("We couldn't send the email"), "body: {body}");
    assert!(body.contains("Try again in a minute."), "body: {body}");
    assert_no_env_var_name("send failed", &body);
}
