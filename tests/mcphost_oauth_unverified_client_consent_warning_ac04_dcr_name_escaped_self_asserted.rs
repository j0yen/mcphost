//! PRD-mcphost-oauth-unverified-client-consent-warning
//! AC4 (P0) — Given a DCR client whose `client_name` is an attacker-supplied
//! string such as `"mcphost Official"` or one containing HTML, When the
//! consent page renders, Then the name is HTML-escaped and presented as the
//! app's self-asserted name, never with verified or first-party styling.

use crate::common;
use common::TestServer;
use serde_json::{Value, json};

async fn register_dcr_client(http: &reqwest::Client, base_url: &str, client_name: &str) -> String {
    let register: Value = http
        .post(format!("{base_url}/oauth/register"))
        .json(&json!({
            "application_type": "web",
            "redirect_uris": ["https://consumer.example/callback"],
            "client_name": client_name,
        }))
        .send()
        .await
        .expect("POST /oauth/register")
        .json()
        .await
        .expect("parse register response");
    register["client_id"].as_str().expect("client_id").to_string()
}

fn authorize_url(base_url: &str, client_id: &str) -> String {
    let mut url = reqwest::Url::parse(&format!("{base_url}/oauth/authorize")).expect("parse base authorize url");
    url.query_pairs_mut()
        .append_pair("response_type", "code")
        .append_pair("client_id", client_id)
        .append_pair("redirect_uri", "https://consumer.example/callback")
        .append_pair("code_challenge", "dummy-challenge")
        .append_pair("code_challenge_method", "S256")
        .append_pair("state", "abc")
        .append_pair("scope", "mcp")
        .append_pair("resource", &format!("{base_url}/mcp"));
    url.to_string()
}

#[tokio::test]
async fn attacker_chosen_impersonating_name_renders_escaped_and_self_asserted() {
    let server = TestServer::start().await;
    let http = reqwest::Client::new();
    let client_id = register_dcr_client(&http, &server.base_url, "mcphost Official").await;

    let resp = reqwest::get(authorize_url(&server.base_url, &client_id)).await.expect("GET /oauth/authorize");
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    let body = resp.text().await.expect("body");

    assert!(body.contains("mcphost Official"), "the self-asserted name itself must still render: {body}");
    assert!(
        body.contains("self-reported name") || body.contains("self-asserted"),
        "the name must be labeled as self-reported/self-asserted, not first-party: {body}"
    );
    assert!(
        body.to_lowercase().contains("unverified"),
        "an attacker-chosen impersonating name must still render inside the unverified caution, never styled as verified/first-party: {body}"
    );
}

#[tokio::test]
async fn html_containing_client_name_is_escaped_not_injected() {
    let server = TestServer::start().await;
    let http = reqwest::Client::new();
    let malicious_name = "<script>alert('pwn')</script><b>Evil</b>";
    let client_id = register_dcr_client(&http, &server.base_url, malicious_name).await;

    let resp = reqwest::get(authorize_url(&server.base_url, &client_id)).await.expect("GET /oauth/authorize");
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    let body = resp.text().await.expect("body");

    assert!(
        !body.contains("<script>alert('pwn')</script>"),
        "raw HTML from client_name must never appear unescaped: {body}"
    );
    assert!(!body.contains("<b>Evil</b>"), "raw HTML from client_name must never appear unescaped: {body}");
    assert!(
        body.contains("&lt;script&gt;") && body.contains("&lt;b&gt;"),
        "the HTML must render HTML-escaped instead: {body}"
    );
    assert!(
        body.contains("self-reported name") || body.contains("self-asserted"),
        "even a malicious name must be labeled self-reported, never first-party: {body}"
    );
}
