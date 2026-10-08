//! PRD-mcphost-oauth-first-grant-signup
//! AC5 (P0) — Given the signup pause file is present, When the choice is
//! submitted, Then no tenant is created and the page shows `signup_paused`
//! within 1 s.

use crate::common;
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use common::{McpClient, TestServer};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

const VERIFIER: &str = "a-pkce-verifier-at-least-43-chars-long-for-realism";

fn code_challenge_for(verifier: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(verifier.as_bytes());
    URL_SAFE_NO_PAD.encode(hasher.finalize())
}

fn http() -> reqwest::Client {
    reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).build().unwrap()
}

async fn register(http: &reqwest::Client, base_url: &str) -> String {
    let body: Value = http
        .post(format!("{base_url}/oauth/register"))
        .json(&json!({"application_type": "native", "redirect_uris": ["http://127.0.0.1/cb"]}))
        .send()
        .await
        .expect("register")
        .json()
        .await
        .expect("register json");
    body["client_id"].as_str().expect("client_id").to_string()
}

/// POSTs the consent form with `extra` fields appended.
async fn consent(http: &reqwest::Client, base_url: &str, client_id: &str, extra: &[(&str, &str)]) -> reqwest::Response {
    let resource = format!("{base_url}/mcp");
    let challenge = code_challenge_for(VERIFIER);
    let mut form = vec![
        ("response_type", "code"),
        ("client_id", client_id),
        ("redirect_uri", "http://127.0.0.1/cb"),
        ("code_challenge", challenge.as_str()),
        ("code_challenge_method", "S256"),
        ("state", "abc"),
        ("scope", "mcp"),
        ("resource", resource.as_str()),
    ];
    form.extend_from_slice(extra);
    http.post(format!("{base_url}/oauth/authorize")).form(&form).send().await.expect("consent")
}

async fn tenant_count(server: &TestServer) -> usize {
    let admin = McpClient::with_bearer(&server.base_url, common::ADMIN_KEY);
    let out = admin.tools_call("admin.tenants", json!({})).await.expect("admin.tenants");
    common::extract_structured(&out)["tenants"].as_array().map_or(0, Vec::len)
}

#[tokio::test]
async fn new_workspace_refused_with_signup_paused_while_pause_file_exists() {
    let server = TestServer::start().await;
    let http = http();
    let client_id = register(&http, &server.base_url).await;
    std::fs::write(server.state.signup_pause.path(), "paused for maintenance\n").expect("write pause file");

    let resp = tokio::time::timeout(
        std::time::Duration::from_secs(1),
        consent(&http, &server.base_url, &client_id, &[("new_workspace", "1")]),
    )
    .await
    .expect("the signup_paused refusal must arrive within 1 s");
    assert!(!resp.status().is_redirection(), "paused: no code may be issued");
    let body = resp.text().await.unwrap();
    assert!(body.contains("signup_paused"), "page must show signup_paused: {body}");
    assert_eq!(tenant_count(&server).await, 0, "no tenant is created while paused");
}
