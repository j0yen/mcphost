//! PRD-mcphost-hosted-authorization-server
//! AC5 (P0) — Given a redeemed code, When `POST /oauth/token` presents it
//! again, Then `invalid_grant` and the previously issued access and
//! refresh tokens are rejected within 60 s.

use crate::common;
use common::{McpClient, TestServer, signup};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use sha2::{Digest, Sha256};
use serde_json::{Value, json};

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
async fn replayed_code_is_invalid_grant_and_revokes_previously_issued_tokens() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Replay Tenant").await;
    let http = reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).build().unwrap();

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
    let resource = format!("{}/mcp", server.base_url);

    let consent_resp = http
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
            ("tenant_key", key.as_str()),
        ])
        .send()
        .await
        .expect("POST /oauth/authorize");
    let location = consent_resp.headers().get("location").expect("Location").to_str().unwrap().to_string();
    let (_, query) = location.split_once('?').expect("query string");
    let code = query_param(query, "code").expect("code").to_string();

    let token_form = [
        ("grant_type", "authorization_code"),
        ("code", code.as_str()),
        ("redirect_uri", "http://127.0.0.1/cb"),
        ("client_id", client_id.as_str()),
        ("code_verifier", verifier),
    ];

    let first: Value = http
        .post(format!("{}/oauth/token", server.base_url))
        .form(&token_form)
        .send()
        .await
        .expect("first token exchange")
        .json()
        .await
        .expect("parse first token response");
    let access_token = first["access_token"].as_str().expect("access_token").to_string();
    let refresh_token = first["refresh_token"].as_str().expect("refresh_token").to_string();

    // The access token works before the replay.
    let bearer_client = McpClient::with_bearer(&server.base_url, &access_token);
    bearer_client
        .tools_call("host.whoami", json!({}))
        .await
        .expect("access token must work before replay");

    // Replay: the same code presented again.
    let second_resp = http
        .post(format!("{}/oauth/token", server.base_url))
        .form(&token_form)
        .send()
        .await
        .expect("second token exchange");
    assert_eq!(second_resp.status(), reqwest::StatusCode::BAD_REQUEST);
    let second: Value = second_resp.json().await.expect("parse second token response");
    assert_eq!(second["error"], json!("invalid_grant"), "replay body: {second}");

    // The previously issued access token must now be rejected.
    let err = bearer_client
        .tools_call("host.whoami", json!({}))
        .await
        .expect_err("access token must be rejected after code replay");
    assert_eq!(err.error_code.as_deref(), Some("invalid_token"), "err: {err:?}");

    // The previously issued refresh token must now be rejected too.
    let refresh_resp = http
        .post(format!("{}/oauth/token", server.base_url))
        .form(&[("grant_type", "refresh_token"), ("refresh_token", refresh_token.as_str())])
        .send()
        .await
        .expect("refresh attempt");
    assert_eq!(refresh_resp.status(), reqwest::StatusCode::BAD_REQUEST);
    let refresh_body: Value = refresh_resp.json().await.expect("parse refresh response");
    assert_eq!(refresh_body["error"], json!("invalid_grant"), "refresh body: {refresh_body}");
}
