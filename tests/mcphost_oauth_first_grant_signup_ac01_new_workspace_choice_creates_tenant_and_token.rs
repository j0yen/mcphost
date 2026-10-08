//! PRD-mcphost-oauth-first-grant-signup
//! AC1 (P0) — Given a DCR client starts `/oauth/authorize` and the user posts
//! the consent form with neither `tenant_key` nor `claim_code`, When the "new
//! workspace" choice is submitted, Then a tenant exists with `source:
//! "oauth"`, the authorization code is issued, and the token exchange
//! succeeds.

use crate::common;
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use common::{McpClient, TestServer};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

fn code_challenge_for(verifier: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(verifier.as_bytes());
    URL_SAFE_NO_PAD.encode(hasher.finalize())
}

fn query_param<'a>(query: &'a str, name: &str) -> Option<&'a str> {
    query.split('&').find_map(|pair| {
        let (k, v) = pair.split_once('=')?;
        (k == name).then_some(v)
    })
}

#[tokio::test]
async fn new_workspace_choice_creates_oauth_tenant_issues_code_and_token() {
    let server = TestServer::start().await;
    let http = reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).build().unwrap();
    let resource = format!("{}/mcp", server.base_url);

    let register: Value = http
        .post(format!("{}/oauth/register", server.base_url))
        .json(&json!({"application_type": "native", "redirect_uris": ["http://127.0.0.1/cb"]}))
        .send()
        .await
        .expect("POST /oauth/register")
        .json()
        .await
        .expect("parse register response");
    let client_id = register["client_id"].as_str().expect("client_id").to_string();
    let verifier = "a-pkce-verifier-at-least-43-chars-long-for-realism";
    let challenge = code_challenge_for(verifier);

    // The consent page offers the third form.
    let page = http
        .get(format!("{}/oauth/authorize", server.base_url))
        .query(&[
            ("response_type", "code"),
            ("client_id", client_id.as_str()),
            ("redirect_uri", "http://127.0.0.1/cb"),
            ("code_challenge", challenge.as_str()),
            ("code_challenge_method", "S256"),
            ("state", "abc"),
            ("scope", "mcp"),
            ("resource", resource.as_str()),
        ])
        .send()
        .await
        .expect("GET /oauth/authorize")
        .text()
        .await
        .expect("consent page body");
    assert!(page.contains("Create a new workspace"), "consent page must offer the new-workspace choice: {page}");

    let consent = http
        .post(format!("{}/oauth/authorize", server.base_url))
        .form(&[
            ("response_type", "code"),
            ("client_id", client_id.as_str()),
            ("redirect_uri", "http://127.0.0.1/cb"),
            ("code_challenge", challenge.as_str()),
            ("code_challenge_method", "S256"),
            ("state", "abc"),
            ("scope", "mcp"),
            ("resource", resource.as_str()),
            ("new_workspace", "1"),
        ])
        .send()
        .await
        .expect("POST /oauth/authorize");
    assert!(consent.status().is_redirection(), "new-workspace consent must redirect: {}", consent.status());
    let location = consent.headers().get("location").expect("Location").to_str().unwrap().to_string();
    let (_, query) = location.split_once('?').expect("query string");
    let code = query_param(query, "code").expect("authorization code issued").to_string();

    let token: Value = http
        .post(format!("{}/oauth/token", server.base_url))
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", code.as_str()),
            ("redirect_uri", "http://127.0.0.1/cb"),
            ("client_id", client_id.as_str()),
            ("code_verifier", verifier),
        ])
        .send()
        .await
        .expect("token exchange")
        .json()
        .await
        .expect("parse token response");
    let access_token = token["access_token"].as_str().unwrap_or_else(|| panic!("no access_token: {token}"));

    let whoami = McpClient::with_bearer(&server.base_url, access_token)
        .tools_call("host.whoami", json!({}))
        .await
        .expect("host.whoami with the issued token");
    let who = common::extract_structured(&whoami);
    assert_eq!(who["source"], json!("oauth"), "the new tenant must be source oauth: {who}");
    assert!(who["display_name"].as_str().is_some_and(|n| n.starts_with("agent-")), "agent-<ulid> name: {who}");
}
