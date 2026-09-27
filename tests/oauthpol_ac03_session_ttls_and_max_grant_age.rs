//! PRD-mcphost-oauth-client-policy
//! AC3 (P0) — Given `access_ttl_s: 300, refresh_ttl_s: 3600,
//! max_grant_age_s: 7200`, When a token is minted, Then `expires_in ==
//! 300`, the refresh token expires after 3600 s, and a refresh after 7200
//! s from consent returns `invalid_grant` `grant_expired`.

use crate::common;
use common::{McpClient, TestServer, signup};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
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
async fn ttls_apply_and_grant_expires_at_max_grant_age() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "TTL Tenant").await;
    let tenant_client = McpClient::with_bearer(&server.base_url, &key);
    let http = reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).build().unwrap();
    let resource = format!("{}/mcp", server.base_url);

    tenant_client
        .tools_call(
            "host.oauth.policy_set",
            json!({"access_ttl_s": 300, "refresh_ttl_s": 3600, "max_grant_age_s": 7200}),
        )
        .await
        .expect("policy_set");

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
            ("tenant_key", key.as_str()),
        ])
        .send()
        .await
        .expect("POST /oauth/authorize");
    assert!(consent.status().is_redirection());
    let location = consent.headers().get("location").unwrap().to_str().unwrap().to_string();
    let (_base, query) = location.split_once('?').unwrap();
    let code = query_param(query, "code").expect("code present").to_string();

    let token_resp: Value = http
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
        .expect("POST /oauth/token")
        .json()
        .await
        .expect("parse token response");
    assert_eq!(token_resp["expires_in"], json!(300), "access_ttl_s must set expires_in");
    let refresh_token = token_resp["refresh_token"].as_str().expect("refresh_token").to_string();

    // The refresh token's own stored TTL is exactly refresh_ttl_s.
    let refresh_hash = mcphost::auth::hash_key(&refresh_token);
    let refresh_row = server
        .state
        .db
        .find_oauth_refresh_token_by_hash(refresh_hash)
        .await
        .expect("query refresh token")
        .expect("refresh token row exists");
    assert_eq!(
        refresh_row.expires_unix - refresh_row.created_unix,
        3600,
        "refresh_ttl_s must set the refresh token's own TTL"
    );

    // Backdate the grant's consent time by just over max_grant_age_s so a
    // refresh now reads grant_expired -- without a real 7200s wait.
    let grants = tenant_client.tools_call("host.oauth.grants", json!({})).await.expect("host.oauth.grants");
    let grant_id = common::extract_structured(&grants)["grants"][0]["id"].as_i64().expect("grant id");
    let backdated = mcphost::state::now_unix() - 7201;
    server
        .state
        .db
        .set_oauth_grant_created_unix_for_test(grant_id, backdated)
        .await
        .expect("backdate grant");

    let refresh_resp: Value = http
        .post(format!("{}/oauth/token", server.base_url))
        .form(&[("grant_type", "refresh_token"), ("refresh_token", refresh_token.as_str())])
        .send()
        .await
        .expect("POST /oauth/token refresh")
        .json()
        .await
        .expect("parse refresh response");
    assert_eq!(refresh_resp["error"], json!("invalid_grant"));
    assert_eq!(refresh_resp["error_description"], json!("grant_expired"));
}
