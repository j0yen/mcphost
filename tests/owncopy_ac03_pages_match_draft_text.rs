//! PRD-mcphost-ownership-copy
//! AC3 (P0) — Given the three pages rendered with sample data, When their
//! text is compared to the draft, Then titles, leads, button label,
//! masked address, counts and closing lines match, and the done page
//! shows the three "what owning it gives you" lines.

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
async fn the_three_pages_match_the_approved_draft() {
    let fake = Arc::new(mcphost::email::FakeEmailClient::new());
    let server = TestServer::start_with_email(fake.clone()).await;
    let anon = McpClient::new(&server.base_url);

    let raw = anon
        .tools_call("signup", json!({"name": "my-agent"}))
        .await
        .expect("signup");
    let result = extract_structured(&raw);
    let token = token_from_claim_url(result["claim_url"].as_str().expect("claim_url")).to_string();
    let key = result["key"].as_str().expect("key").to_string();

    let tenant_client = McpClient::with_bearer(&server.base_url, &key);
    tenant_client
        .tools_call(
            "host.tool_publish",
            json!({"name": "pinger", "kind": "echo", "spec": {"schema": {"type": "object"}}}),
        )
        .await
        .expect("publish");

    let http = reqwest::Client::new();

    // Page 1: enter your address.
    let enter_page = http
        .get(format!("{}/claim/{token}", server.base_url))
        .send()
        .await
        .expect("GET /claim/{token}")
        .text()
        .await
        .expect("body");
    assert!(enter_page.contains("Make my-agent yours"), "title/h1: {enter_page}");
    assert!(
        enter_page.contains(
            "An agent set up \"my-agent\" on mcphost. Enter your email and we'll send a one-time link that makes you its owner."
        ),
        "lead: {enter_page}"
    );
    assert!(enter_page.contains("you@example.com"), "placeholder: {enter_page}");
    assert!(enter_page.contains("Send me the link"), "button label: {enter_page}");
    assert!(
        enter_page.contains("We send one email and store the address only as the owner of this backend."),
        "footnote: {enter_page}"
    );

    // Page 2: after sending.
    let after_send_page = http
        .post(format!("{}/claim/{token}", server.base_url))
        .form(&[("email", "jsmith@gmail.com")])
        .send()
        .await
        .expect("POST /claim/{token}")
        .text()
        .await
        .expect("body");
    assert!(
        after_send_page.contains("One more step: open your email"),
        "title/h1: {after_send_page}"
    );
    assert!(after_send_page.contains("j***@gmail.com"), "masked address: {after_send_page}");
    assert!(
        after_send_page.contains("It works once and stops working in 30 minutes."),
        "30-minute note: {after_send_page}"
    );
    assert!(after_send_page.contains("send it again"), "resend link: {after_send_page}");

    let sends = fake.sends();
    assert_eq!(sends.len(), 1);
    let code = code_from_email_body(&sends[0].text_body).to_string();

    // Page 3: done.
    let done_page = http
        .get(format!("{}/claim/verify/{code}", server.base_url))
        .send()
        .await
        .expect("GET /claim/verify/{code}")
        .text()
        .await
        .expect("body");
    assert!(done_page.contains("my-agent is yours"), "title/h1: {done_page}");
    assert!(
        done_page.contains("Signed in as") && done_page.contains("jsmith@gmail.com"),
        "lead: {done_page}"
    );
    assert!(
        done_page.contains("Your agent keeps working exactly as before; you now hold the keys."),
        "lead: {done_page}"
    );
    assert!(done_page.contains("1 tools"), "tool count: {done_page}");
    assert!(done_page.contains("0 schedules"), "schedule count: {done_page}");
    assert!(done_page.contains("0 webhooks"), "webhook count: {done_page}");
    assert!(done_page.contains("calls this month"), "calls count: {done_page}");
    assert!(done_page.contains("bytes of state"), "state count: {done_page}");
    assert!(
        done_page.contains("see everything the agent builds"),
        "owning-it line 1: {done_page}"
    );
    assert!(
        done_page.contains("get a new key if the agent loses its own"),
        "owning-it line 2: {done_page}"
    );
    assert!(
        done_page.contains("choose a plan when the free tier stops being enough"),
        "owning-it line 3: {done_page}"
    );
    assert!(
        done_page.contains("Nothing else to do. You can close this page."),
        "closing line: {done_page}"
    );
}
