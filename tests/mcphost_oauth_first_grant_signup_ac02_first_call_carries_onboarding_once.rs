//! PRD-mcphost-oauth-first-grant-signup
//! AC2 (P0) — Given that token, When `host.whoami` is called, Then it returns
//! the new tenant, `source: "oauth"`, and the result envelope carries
//! `onboarding.url` and `onboarding.invite_url`; a second call carries no
//! `onboarding`.

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

/// Registers a DCR client, posts the new-workspace consent, exchanges the
/// code; returns (client_id, access_token).
async fn new_workspace_token(base_url: &str) -> (String, String) {
    let http = reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).build().unwrap();
    let resource = format!("{base_url}/mcp");
    let register: Value = http
        .post(format!("{base_url}/oauth/register"))
        .json(&json!({"application_type": "native", "redirect_uris": ["http://127.0.0.1/cb"]}))
        .send()
        .await
        .expect("register")
        .json()
        .await
        .expect("register json");
    let client_id = register["client_id"].as_str().expect("client_id").to_string();
    let verifier = "a-pkce-verifier-at-least-43-chars-long-for-realism";
    let challenge = code_challenge_for(verifier);
    let consent = http
        .post(format!("{base_url}/oauth/authorize"))
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
        .expect("consent");
    let location = consent.headers().get("location").expect("Location").to_str().unwrap().to_string();
    let (_, query) = location.split_once('?').expect("query");
    let code = query
        .split('&')
        .find_map(|p| p.strip_prefix("code="))
        .expect("code")
        .to_string();
    let token: Value = http
        .post(format!("{base_url}/oauth/token"))
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", code.as_str()),
            ("redirect_uri", "http://127.0.0.1/cb"),
            ("client_id", client_id.as_str()),
            ("code_verifier", verifier),
        ])
        .send()
        .await
        .expect("token")
        .json()
        .await
        .expect("token json");
    (client_id, token["access_token"].as_str().unwrap_or_else(|| panic!("{token}")).to_string())
}

#[tokio::test]
async fn first_whoami_carries_onboarding_second_does_not() {
    let server = TestServer::start().await;
    let (_client_id, access_token) = new_workspace_token(&server.base_url).await;
    let client = McpClient::with_bearer(&server.base_url, &access_token);

    let first = common::extract_structured(&client.tools_call("host.whoami", json!({})).await.expect("first whoami"));
    assert_eq!(first["source"], json!("oauth"), "{first}");
    let tenant = first["tenant"].as_str().expect("tenant").to_string();
    let onboarding = &first["onboarding"];
    assert_eq!(onboarding["tenant"], json!(tenant), "{first}");
    assert!(onboarding["url"].as_str().is_some_and(|u| u.contains("/u/") && u.ends_with("/mcp")), "{first}");
    assert!(onboarding["invite_url"].as_str().is_some_and(|u| u.contains("/i/")), "{first}");
    assert!(onboarding["note"].is_string(), "{first}");

    let second = common::extract_structured(&client.tools_call("host.whoami", json!({})).await.expect("second whoami"));
    assert_eq!(second["tenant"], json!(tenant), "{second}");
    assert!(second.get("onboarding").is_none(), "second call must not carry onboarding: {second}");
}
