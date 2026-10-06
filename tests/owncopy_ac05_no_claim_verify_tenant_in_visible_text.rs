//! PRD-mcphost-ownership-copy
//! AC5 (P0) — Given the guard test over the email and every page, When it
//! runs, Then no visible text contains `claim`, `verify` or `tenant`
//! (case-insensitive) outside URLs and attributes.

use crate::common;
use common::{McpClient, TestServer, extract_structured};
use serde_json::json;
use std::sync::Arc;

const FORBIDDEN: [&str; 3] = ["claim", "verify", "tenant"];

/// Strips HTML tags (and, with them, every attribute -- `href`/`action`
/// values live inside a tag and are never captured) and leaves whatever
/// text sat between `>` and the next `<`, including `<title>`'s own
/// (visible in a browser tab, so in scope).
fn strip_tags(html: &str) -> String {
    let mut out = String::new();
    let mut in_tag = false;
    for c in html.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    out
}

/// A URL is allowed to carry "claim"/"verify" in its path (the open
/// question's own "keep `/claim/<token>`" decision); checked per
/// whitespace-separated token so a URL embedded in prose (the plain-text
/// email body) doesn't fail the scan around it.
fn assert_no_forbidden_prose(label: &str, text: &str) {
    for token in text.split_whitespace() {
        if token.contains("://") {
            continue;
        }
        let lower = token.to_lowercase();
        for needle in FORBIDDEN {
            assert!(
                !lower.contains(needle),
                "{label}: forbidden word '{needle}' in visible text token '{token}' (full text: {text})"
            );
        }
    }
}

fn token_from_claim_url(claim_url: &str) -> &str {
    claim_url.rsplit('/').next().expect("claim_url has a path segment")
}

fn code_from_email_body(body: &str) -> &str {
    let idx = body.find("/claim/verify/").expect("verify URL in email body");
    let after = &body[idx + "/claim/verify/".len()..];
    after.split_whitespace().next().expect("code token")
}

#[tokio::test]
async fn no_visible_text_in_the_email_or_any_page_says_claim_verify_or_tenant() {
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

    // The enter-address page.
    let enter_page = http
        .get(format!("{}/claim/{token}", server.base_url))
        .send()
        .await
        .expect("GET /claim/{token}")
        .text()
        .await
        .expect("body");
    assert_no_forbidden_prose("enter-address page", &strip_tags(&enter_page));

    // The after-send page.
    let after_send_page = http
        .post(format!("{}/claim/{token}", server.base_url))
        .form(&[("email", "owner@example.com")])
        .send()
        .await
        .expect("POST /claim/{token}")
        .text()
        .await
        .expect("body");
    assert_no_forbidden_prose("after-send page", &strip_tags(&after_send_page));

    // The ownership email itself: subject, then the plain-text body (its
    // own verify_url line is a URL, skipped token-by-token above).
    let sends = fake.sends();
    assert_eq!(sends.len(), 1);
    let message = &sends[0];
    assert_no_forbidden_prose("email subject", &message.subject);
    assert_no_forbidden_prose("email body", &message.text_body);

    // The done page.
    let code = code_from_email_body(&message.text_body).to_string();
    let done_page = http
        .get(format!("{}/claim/verify/{code}", server.base_url))
        .send()
        .await
        .expect("GET /claim/verify/{code}")
        .text()
        .await
        .expect("body");
    assert_no_forbidden_prose("done page", &strip_tags(&done_page));

    // The seven error pages.
    let unconfigured_server = TestServer::start().await;
    let unconfigured_client = McpClient::new(&unconfigured_server.base_url);
    let unconfigured_raw = unconfigured_client
        .tools_call("signup", json!({"name": "unconfigured-agent"}))
        .await
        .expect("signup");
    let unconfigured_token = token_from_claim_url(
        extract_structured(&unconfigured_raw)["claim_url"]
            .as_str()
            .expect("claim_url"),
    )
    .to_string();
    let mail_not_configured_page = http
        .get(format!("{}/claim/{unconfigured_token}", unconfigured_server.base_url))
        .send()
        .await
        .expect("GET /claim/{token} (mail not configured)")
        .text()
        .await
        .expect("body");
    assert_no_forbidden_prose("mail-not-configured page", &strip_tags(&mail_not_configured_page));

    server
        .state
        .db
        .expire_claim_token_for_test(mcphost::auth::hash_key(&token))
        .await
        .expect("expire_claim_token_for_test");
    let expired_page = http
        .get(format!("{}/claim/{token}", server.base_url))
        .send()
        .await
        .expect("GET /claim/{token} (expired)")
        .text()
        .await
        .expect("body");
    assert_no_forbidden_prose("expired page", &strip_tags(&expired_page));

    http.get(format!("{}/claim/verify/{code}", server.base_url))
        .send()
        .await
        .expect("first GET /claim/verify/{code}");
    let already_used_page = http
        .get(format!("{}/claim/verify/{code}", server.base_url))
        .send()
        .await
        .expect("second GET /claim/verify/{code}")
        .text()
        .await
        .expect("body");
    assert_no_forbidden_prose("already-used page", &strip_tags(&already_used_page));

    let fake2 = Arc::new(mcphost::email::FakeEmailClient::new());
    let server2 = TestServer::start_with_email(fake2.clone()).await;
    let client2 = McpClient::new(&server2.base_url);
    let raw2 = client2
        .tools_call("signup", json!({"name": "conflict-agent-2"}))
        .await
        .expect("signup");
    let token2 = token_from_claim_url(
        extract_structured(&raw2)["claim_url"].as_str().expect("claim_url"),
    )
    .to_string();
    http.post(format!("{}/claim/{token2}", server2.base_url))
        .form(&[("email", "first@example.com")])
        .send()
        .await
        .expect("first POST /claim/{token}");
    http.post(format!("{}/claim/{token2}", server2.base_url))
        .form(&[("email", "second@example.com")])
        .send()
        .await
        .expect("second POST /claim/{token}");
    let sends2 = fake2.sends();
    assert_eq!(sends2.len(), 2);
    let code_a = code_from_email_body(&sends2[0].text_body).to_string();
    let code_b = code_from_email_body(&sends2[1].text_body).to_string();
    let base2 = server2.base_url.clone();
    let (resp_a, resp_b) = tokio::join!(
        http.get(format!("{base2}/claim/verify/{code_a}")).send(),
        http.get(format!("{base2}/claim/verify/{code_b}")).send(),
    );
    let resp_a = resp_a.expect("verify a");
    let resp_b = resp_b.expect("verify b");
    let loser = if resp_a.status() == reqwest::StatusCode::CONFLICT {
        resp_a
    } else {
        resp_b
    };
    let conflict_page = loser.text().await.expect("body");
    assert_no_forbidden_prose("someone-else-first page", &strip_tags(&conflict_page));

    let rl_server = TestServer::start().await;
    let rl_client = McpClient::new(&rl_server.base_url);
    let rl_raw = rl_client
        .tools_call("signup", json!({"name": "ratelimit-agent"}))
        .await
        .expect("signup");
    let rl_token = token_from_claim_url(
        extract_structured(&rl_raw)["claim_url"].as_str().expect("claim_url"),
    )
    .to_string();
    let mut rate_limited_page = String::new();
    for _ in 0..31 {
        rate_limited_page = http
            .get(format!("{}/claim/{rl_token}", rl_server.base_url))
            .send()
            .await
            .expect("GET /claim/{token}")
            .text()
            .await
            .expect("body");
    }
    assert_no_forbidden_prose("rate-limited page", &strip_tags(&rate_limited_page));

    let ban_server = TestServer::start().await;
    let addr = "203.0.113.88";
    let since = mcphost::state::now_unix() - 600;
    for _ in 0..30 {
        ban_server
            .state
            .db
            .try_admit_claim_request(addr.to_string(), since, 1000)
            .await
            .expect("try_admit_claim_request");
    }
    mcphost::bans::tick_once(&ban_server.state).await.expect("tick_once");
    let banned_page = http
        .get(format!("{}/claim/verify/nonexistent-code", ban_server.base_url))
        .header("x-forwarded-for", addr)
        .send()
        .await
        .expect("GET /claim/verify/{code}")
        .text()
        .await
        .expect("body");
    assert_no_forbidden_prose("banned page", &strip_tags(&banned_page));

    let fail_fake = Arc::new(mcphost::email::FakeEmailClient::new());
    fail_fake.fail_next(2);
    let fail_server = TestServer::start_with_email(fail_fake).await;
    let fail_client = McpClient::new(&fail_server.base_url);
    let fail_raw = fail_client
        .tools_call("signup", json!({"name": "sendfail-agent"}))
        .await
        .expect("signup");
    let fail_token = token_from_claim_url(
        extract_structured(&fail_raw)["claim_url"].as_str().expect("claim_url"),
    )
    .to_string();
    let send_error_page = http
        .post(format!("{}/claim/{fail_token}", fail_server.base_url))
        .form(&[("email", "a@b.co")])
        .send()
        .await
        .expect("POST /claim/{token}")
        .text()
        .await
        .expect("body");
    assert_no_forbidden_prose("send-error page", &strip_tags(&send_error_page));
}
