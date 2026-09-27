//! PRD-mcphost-enterprise-managed-auth
//! AC9 (P0) — Given the hosted AS PRD's interactive suites and the
//! key-based suites, When run, Then they pass unchanged.
//!
//! The interactive/key-based suites themselves (`hostedas_ac0*.rs`,
//! `ac0*.rs`, etc.) are the real proof here and this branch's full `cargo
//! test` run staying green is what AC9 actually asks for. This test adds a
//! targeted regression guard on the two call paths this PRD's refactor
//! touched directly: `validate_hosted_bearer` (generalized to resolve
//! tenant/audience from the jti row instead of a fixed root resource and a
//! sub-derived tenant) must still discriminate an ordinary hosted token
//! from an assertion-derived one, and `issue_tokens`/`token_refresh_token`
//! (now thin wrappers over `issue_tokens_for_subject`) must still mint and
//! rotate a plain hosted grant's refresh token exactly as before.

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

#[tokio::test]
async fn hosted_token_and_key_based_calls_still_work_after_the_jwt_bearer_refactor() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "Hosted AS Tenant").await;
    let http = reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).build().unwrap();

    // Key-based calls are untouched by this PRD -- no OAuth involved at all.
    let key_client = McpClient::with_bearer(&server.base_url, &key);
    let whoami_by_key = key_client.tools_call("host.whoami", json!({})).await.expect("key-based host.whoami");
    assert_eq!(common::extract_structured(&whoami_by_key)["tenant"], json!(ns));

    // The interactive authorization_code + refresh_token dance still mints
    // an ordinary hosted token, not an assertion-derived one.
    let register: Value = http
        .post(format!("{}/oauth/register", server.base_url))
        .json(&json!({"application_type": "native", "redirect_uris": [REDIRECT_URI]}))
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
            ("redirect_uri", REDIRECT_URI),
            ("code_challenge", challenge.as_str()),
            ("code_challenge_method", "S256"),
            ("state", "xyz"),
            ("scope", "mcp"),
            ("resource", resource.as_str()),
            ("tenant_key", key.as_str()),
        ])
        .send()
        .await
        .expect("POST /oauth/authorize");
    let location = consent_resp.headers().get("location").expect("Location header").to_str().unwrap().to_string();
    let (_base, query) = location.split_once('?').expect("redirect must carry a query string");
    let code = query_param(query, "code").expect("code present in redirect").to_string();

    let token_resp: Value = http
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
        .expect("POST /oauth/token")
        .json()
        .await
        .expect("parse token response");
    let access_token = token_resp["access_token"].as_str().expect("access_token present").to_string();
    let refresh_token = token_resp["refresh_token"].as_str().expect("refresh_token present").to_string();

    let bearer_client = McpClient::with_bearer(&server.base_url, &access_token);
    let whoami_by_token = bearer_client.tools_call("host.whoami", json!({})).await.expect("hosted-token host.whoami");
    let whoami_structured = common::extract_structured(&whoami_by_token);
    assert_eq!(whoami_structured["tenant"], json!(ns));
    assert_eq!(whoami_structured["auth_method"], json!("hosted_token"), "must not be misclassified as enterprise_assertion");

    // The refresh token still rotates and works exactly once.
    let refreshed: Value = http
        .post(format!("{}/oauth/token", server.base_url))
        .form(&[("grant_type", "refresh_token"), ("refresh_token", refresh_token.as_str())])
        .send()
        .await
        .expect("POST /oauth/token refresh")
        .json()
        .await
        .expect("parse refresh response");
    assert!(refreshed["access_token"].as_str().is_some(), "refresh must mint a fresh access token: {refreshed}");
    let rotated = refreshed["refresh_token"].as_str().expect("refresh must rotate to a new refresh_token").to_string();
    assert_ne!(rotated, refresh_token);

    let reused: Value = http
        .post(format!("{}/oauth/token", server.base_url))
        .form(&[("grant_type", "refresh_token"), ("refresh_token", refresh_token.as_str())])
        .send()
        .await
        .expect("POST /oauth/token reused refresh")
        .json()
        .await
        .expect("parse reused-refresh response");
    assert_eq!(reused["error"], json!("invalid_grant"), "reusing a rotated refresh_token must still fail: {reused}");
}
