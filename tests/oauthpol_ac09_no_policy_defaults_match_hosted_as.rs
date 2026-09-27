//! PRD-mcphost-oauth-client-policy
//! AC9 (P0) — Given tenants with no policy set, When the hosted AS PRD's
//! suites and the harness run, Then they pass unchanged.
//!
//! This file is the dedicated, always-on proof: a tenant that never calls
//! `host.oauth.policy_set` gets `host.oauth.policy`'s documented defaults
//! (requirement 1: "defaults reproduce the hosted AS PRD's behaviour
//! exactly") and a real authorize -> code -> token -> refresh round trip
//! behaves byte-for-byte as PRD-mcphost-hosted-authorization-server's own
//! `hostedas_ac04`/`hostedas_ac07` suites already prove -- `expires_in ==
//! 3600`, any client is accepted, and a normal (not-yet-expired) refresh
//! still succeeds. The full existing `hostedas_*`/`oauthrs_*` suites
//! themselves are the other half of this AC's proof; this file doesn't
//! duplicate them, it only pins the new module's own default path.

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
async fn a_tenant_with_no_policy_set_gets_the_documented_defaults() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "No-Policy Tenant").await;
    let tenant_client = McpClient::with_bearer(&server.base_url, &key);

    let policy = tenant_client.tools_call("host.oauth.policy", json!({})).await.expect("host.oauth.policy");
    let structured = common::extract_structured(&policy);
    assert_eq!(structured["clients"], json!("any"));
    assert_eq!(structured["allowlist"], json!([]));
    assert_eq!(structured["access_ttl_s"], json!(3600));
    assert_eq!(structured["refresh_ttl_s"], json!(2_592_000));
    assert_eq!(structured["max_grant_age_s"], json!(null));
    assert_eq!(structured["reconsent_after_s"], json!(null));
}

#[tokio::test]
async fn default_policy_authorize_token_refresh_round_trip_is_unchanged() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "No-Policy Round Trip Tenant").await;
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
    assert!(consent.status().is_redirection(), "an unlisted DCR client must still reach consent under the default policy");
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
    assert_eq!(token_resp["expires_in"], json!(3600), "the default access TTL must be unchanged");
    let refresh_token = token_resp["refresh_token"].as_str().expect("refresh_token").to_string();

    let refresh_resp: Value = http
        .post(format!("{}/oauth/token", server.base_url))
        .form(&[("grant_type", "refresh_token"), ("refresh_token", refresh_token.as_str())])
        .send()
        .await
        .expect("POST /oauth/token refresh")
        .json()
        .await
        .expect("parse refresh response");
    assert_eq!(refresh_resp["expires_in"], json!(3600), "a normal refresh under no policy must still succeed");
    assert!(refresh_resp["access_token"].as_str().is_some_and(|t| !t.is_empty()));
}
