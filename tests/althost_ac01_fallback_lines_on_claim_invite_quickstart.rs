//! PRD-mcphost-reachability-alt-host
//! AC1 (P0) — Given `MCPHOST_ALT_PUBLIC_URLS=https://alt.example`, When a
//! claim email, an invite response and `host.quickstart` are rendered,
//! Then each carries the primary link, an `alt:` line with the same path
//! on alt.example, and the allow-steps URL, from one template.

use crate::common;
use common::{McpClient, TestServer, extract_structured};
use serde_json::json;
use std::sync::Arc;

fn token_from_claim_url(claim_url: &str) -> &str {
    claim_url.rsplit('/').next().expect("claim_url has a path segment")
}

#[tokio::test]
async fn claim_email_invite_and_quickstart_all_carry_the_alt_fallback() {
    let fake = Arc::new(mcphost::email::FakeEmailClient::new());
    let server =
        TestServer::start_with_email_and_alts(fake.clone(), vec!["https://alt.example".to_string()])
            .await;
    let client = McpClient::new(&server.base_url);

    // --- claim email ---------------------------------------------------
    let raw = client
        .tools_call("signup", json!({"name": "AC1 Tenant"}))
        .await
        .expect("signup");
    let result = extract_structured(&raw);
    let claim_url = result["claim_url"].as_str().expect("claim_url");
    let token = token_from_claim_url(claim_url);

    let http = reqwest::Client::new();
    http.post(format!("{}/claim/{token}", server.base_url))
        .form(&[("email", "a@b.co")])
        .send()
        .await
        .expect("POST /claim/{token}")
        .error_for_status()
        .expect("200");

    assert_eq!(fake.send_count(), 1);
    let sends = fake.sends();
    let body = &sends[0].text_body;
    assert!(body.contains("alt: https://alt.example/claim/verify/"), "body: {body}");
    assert!(body.contains("/cant-reach"), "body must name the allow-steps URL: {body}");

    // --- host.quickstart --------------------------------------------------
    let key = result["key"].as_str().expect("key");
    let authed = McpClient::with_bearer(&server.base_url, key);
    let quickstart_raw = authed
        .tools_call("host.quickstart", json!({"kind": "echo"}))
        .await
        .expect("host.quickstart");
    let quickstart = extract_structured(&quickstart_raw);
    assert_eq!(quickstart["alt_endpoint"], json!("https://alt.example/mcp"));
    let reachability = quickstart["reachability"].as_str().expect("reachability text");
    assert!(reachability.contains("alt: https://alt.example/mcp"), "{reachability}");
    assert!(reachability.contains("/cant-reach"), "{reachability}");

    // --- invite response -------------------------------------------------
    let invite_raw = authed
        .tools_call("host.invite.create", json!({}))
        .await
        .expect("host.invite.create");
    let invite = extract_structured(&invite_raw);
    let code = invite["code"].as_str().expect("code");
    assert_eq!(invite["alt_url"], json!(format!("https://alt.example/i/{code}/mcp")));
}
