//! PRD-mcphost-tool-scopes-and-consent
//! AC8 (P1) — Given two consents for `read` and one for `read write` in 7
//! days, When `admin.oauth.stats` runs, Then `consents_by_scope_7d ==
//! {read: 3, write: 1}`.

use crate::common;
use common::{ADMIN_KEY, McpClient, TestServer, signup};
use serde_json::{Value, json};

fn code_challenge_for(verifier: &str) -> String {
    use base64::Engine as _;
    use sha2::{Digest, Sha256};
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

fn query_param<'a>(query: &'a str, name: &str) -> Option<&'a str> {
    query.split('&').find_map(|pair| {
        let (k, v) = pair.split_once('=')?;
        (k == name).then_some(v)
    })
}

/// One full consent+token-exchange flow, minting a fresh DCR client each
/// time (so the codes/grants are independent) -- returns the granted
/// `scope` string `host.oauth.grants` will show.
async fn consent(server: &TestServer, ns: &str, key: &str, scope: &str, nonce: &str) {
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
    let verifier = format!("verifier-{nonce}-at-least-43-characters-long-for-pkce");
    let challenge = code_challenge_for(&verifier);
    let resource = mcphost::oauth::canonical_resource_uri(&server.base_url, ns);
    let consent_resp = http
        .post(format!("{}/oauth/authorize", server.base_url))
        .form(&[
            ("response_type", "code"),
            ("client_id", client_id.as_str()),
            ("redirect_uri", "http://127.0.0.1/cb"),
            ("code_challenge", challenge.as_str()),
            ("code_challenge_method", "S256"),
            ("state", "abc"),
            ("scope", scope),
            ("resource", resource.as_str()),
            ("tenant_key", key),
        ])
        .send()
        .await
        .expect("POST /oauth/authorize");
    assert!(consent_resp.status().is_redirection(), "consent must redirect: {}", consent_resp.status());
    let location = consent_resp.headers().get("location").expect("Location header").to_str().unwrap().to_string();
    let (_, query) = location.split_once('?').expect("redirect must carry a query string");
    let code = query_param(query, "code").expect("code present in redirect").to_string();
    let token_resp: Value = http
        .post(format!("{}/oauth/token", server.base_url))
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", code.as_str()),
            ("redirect_uri", "http://127.0.0.1/cb"),
            ("client_id", client_id.as_str()),
            ("code_verifier", verifier.as_str()),
        ])
        .send()
        .await
        .expect("POST /oauth/token")
        .json()
        .await
        .expect("parse token response");
    assert_eq!(token_resp["scope"], json!(scope), "grant must carry the requested scope");
}

#[tokio::test]
async fn two_read_consents_and_one_read_write_consent_tally_by_scope_word() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "Stats Tenant").await;
    let key_client = McpClient::with_bearer(&server.base_url, &key);
    key_client
        .tools_call("host.oauth.scope_set", json!({"name": "read", "description": "Search"}))
        .await
        .expect("scope_set read");
    key_client
        .tools_call(
            "host.oauth.scope_set",
            json!({"name": "write", "description": "Change records"}),
        )
        .await
        .expect("scope_set write");

    consent(&server, &ns, &key, "read", "1").await;
    consent(&server, &ns, &key, "read", "2").await;
    consent(&server, &ns, &key, "read write", "3").await;

    // requirement 6: `host.oauth.grants` rows show the granted scopes too.
    let grants = key_client.tools_call("host.oauth.grants", json!({})).await.expect("host.oauth.grants");
    let grants = common::extract_structured(&grants);
    let scopes: Vec<&str> = grants["grants"]
        .as_array()
        .expect("grants array")
        .iter()
        .map(|g| g["scope"].as_str().expect("grant scope"))
        .collect();
    assert_eq!(scopes.len(), 3, "{scopes:?}");
    assert_eq!(scopes.iter().filter(|s| **s == "read").count(), 2);
    assert_eq!(scopes.iter().filter(|s| **s == "read write").count(), 1);

    let admin = McpClient::with_bearer(&server.base_url, ADMIN_KEY);
    let stats = admin.tools_call("admin.oauth.stats", json!({})).await.expect("admin.oauth.stats");
    let stats = common::extract_structured(&stats);
    assert_eq!(
        stats["consents_by_scope_7d"],
        json!({"read": 3, "write": 1}),
        "{stats:?}"
    );
}
