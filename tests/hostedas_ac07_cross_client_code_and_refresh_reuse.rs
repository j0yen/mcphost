//! PRD-mcphost-hosted-authorization-server
//! AC7 (P0) — Given client A's code, When client B presents it at
//! `/oauth/token`, Then `invalid_grant`; Given a refresh token already
//! rotated, When it is reused, Then `invalid_grant` and the whole grant
//! is revoked.

use crate::common;
use common::{McpClient, TestServer, signup};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use sha2::{Digest, Sha256};
use serde_json::{Value, json};

const REDIRECT_URI: &str = "http://127.0.0.1/cb";

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

async fn register_client(server: &TestServer) -> String {
    let http = reqwest::Client::new();
    let register: Value = http
        .post(format!("{}/oauth/register", server.base_url))
        .json(&json!({"application_type": "native", "redirect_uris": [REDIRECT_URI]}))
        .send()
        .await
        .expect("POST /oauth/register")
        .json()
        .await
        .expect("parse register response");
    register["client_id"].as_str().expect("client_id").to_string()
}

#[tokio::test]
async fn client_b_cannot_redeem_client_as_code() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Cross Client Tenant").await;
    let http = reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).build().unwrap();

    let client_a = register_client(&server).await;
    let client_b = register_client(&server).await;

    let verifier = "a-pkce-verifier-at-least-43-chars-long-for-realism";
    let challenge = code_challenge_for(verifier);
    let resource = format!("{}/mcp", server.base_url);

    let consent_resp = http
        .post(format!("{}/oauth/authorize", server.base_url))
        .form(&[
            ("response_type", "code"),
            ("client_id", client_a.as_str()),
            ("redirect_uri", REDIRECT_URI),
            ("code_challenge", challenge.as_str()),
            ("code_challenge_method", "S256"),
            ("state", "abc"),
            ("scope", "mcp"),
            ("resource", resource.as_str()),
            ("tenant_key", key.as_str()),
        ])
        .send()
        .await
        .expect("POST /oauth/authorize for client A");
    let location = consent_resp.headers().get("location").unwrap().to_str().unwrap().to_string();
    let (_, query) = location.split_once('?').unwrap();
    let code = query_param(query, "code").unwrap().to_string();

    // Client B presents client A's code as its own.
    let resp = http
        .post(format!("{}/oauth/token", server.base_url))
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", code.as_str()),
            ("redirect_uri", REDIRECT_URI),
            ("client_id", client_b.as_str()),
            ("code_verifier", verifier),
        ])
        .send()
        .await
        .expect("client B token exchange");
    assert_eq!(resp.status(), reqwest::StatusCode::BAD_REQUEST);
    let body: Value = resp.json().await.expect("parse response");
    assert_eq!(body["error"], json!("invalid_grant"), "body: {body}");
}

#[tokio::test]
async fn reusing_a_rotated_refresh_token_revokes_the_whole_grant() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Refresh Reuse Tenant").await;
    let http = reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).build().unwrap();
    let client_id = register_client(&server).await;

    let verifier = "a-pkce-verifier-at-least-43-chars-long-for-realism";
    let challenge = code_challenge_for(verifier);
    let resource = format!("{}/mcp", server.base_url);

    let consent_resp = http
        .post(format!("{}/oauth/authorize", server.base_url))
        .form(&[
            ("response_type", "code"),
            ("client_id", client_id.as_str()),
            ("redirect_uri", REDIRECT_URI),
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
    let location = consent_resp.headers().get("location").unwrap().to_str().unwrap().to_string();
    let (_, query) = location.split_once('?').unwrap();
    let code = query_param(query, "code").unwrap().to_string();

    let first: Value = http
        .post(format!("{}/oauth/token", server.base_url))
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", code.as_str()),
            ("redirect_uri", REDIRECT_URI),
            ("client_id", client_id.as_str()),
            ("code_verifier", verifier),
        ])
        .send()
        .await
        .expect("initial token exchange")
        .json()
        .await
        .expect("parse initial token response");
    let refresh_token_1 = first["refresh_token"].as_str().expect("refresh_token").to_string();

    // Rotate once: refresh_token_1 -> refresh_token_2 (+ access_token_2).
    let second: Value = http
        .post(format!("{}/oauth/token", server.base_url))
        .form(&[("grant_type", "refresh_token"), ("refresh_token", refresh_token_1.as_str())])
        .send()
        .await
        .expect("first rotation")
        .json()
        .await
        .expect("parse rotation response");
    let refresh_token_2 = second["refresh_token"].as_str().expect("refresh_token").to_string();
    let access_token_2 = second["access_token"].as_str().expect("access_token").to_string();

    // Reusing the now-rotated refresh_token_1 must fail...
    let reuse_resp = http
        .post(format!("{}/oauth/token", server.base_url))
        .form(&[("grant_type", "refresh_token"), ("refresh_token", refresh_token_1.as_str())])
        .send()
        .await
        .expect("reuse attempt");
    assert_eq!(reuse_resp.status(), reqwest::StatusCode::BAD_REQUEST);
    let reuse_body: Value = reuse_resp.json().await.expect("parse reuse response");
    assert_eq!(reuse_body["error"], json!("invalid_grant"), "body: {reuse_body}");

    // ...and revoke the WHOLE grant: refresh_token_2 (never itself reused)
    // must now also be refused.
    let second_refresh_resp = http
        .post(format!("{}/oauth/token", server.base_url))
        .form(&[("grant_type", "refresh_token"), ("refresh_token", refresh_token_2.as_str())])
        .send()
        .await
        .expect("refresh_token_2 attempt");
    assert_eq!(second_refresh_resp.status(), reqwest::StatusCode::BAD_REQUEST);
    let second_refresh_body: Value = second_refresh_resp.json().await.expect("parse response");
    assert_eq!(second_refresh_body["error"], json!("invalid_grant"), "body: {second_refresh_body}");

    // ...and access_token_2 (minted from that same grant) must also fail.
    let bearer_client = McpClient::with_bearer(&server.base_url, &access_token_2);
    let err = bearer_client
        .tools_call("host.whoami", json!({}))
        .await
        .expect_err("access token from a revoked grant must be rejected");
    assert_eq!(err.error_code.as_deref(), Some("invalid_token"), "err: {err:?}");
}
