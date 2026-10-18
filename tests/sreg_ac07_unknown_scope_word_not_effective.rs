//! PRD-mcphost-scope-registry-generated
//! AC7 (P0) — Given a scope string not in the registry nor the catalog in a
//! token request, When the token is minted, Then the word is dropped exactly
//! as today and `scopes.effective` omits it.

use crate::common;
use common::{McpClient, TestServer, signup};
use serde_json::{Value, json};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
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

/// Runs the full consent+token flow (same shape as AC1's own test) for a
/// tenant with a `{read, write}` catalog and `search`/`delete_records`
/// tools, returning the hosted access token minted for `scope`.
async fn consent_and_get_token(server: &TestServer, ns: &str, key: &str, scope: &str) -> String {
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

    let verifier = format!("verifier-{scope}-at-least-43-chars-long-for-pkce-realism");
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
    token_resp["access_token"].as_str().expect("access_token present").to_string()
}

#[tokio::test]
async fn sreg_ac07_word_outside_registry_and_catalog_is_not_effective() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "Unknown Scope Tenant").await;
    let key_client = McpClient::with_bearer(&server.base_url, &key);
    // `offline_access` is accepted at mint but is in neither the registry nor
    // the catalog: it rides in the token's claim and is omitted from
    // `scopes.effective`, exactly as the step-up union already omits it.
    let token = consent_and_get_token(&server, &ns, &key, "read offline_access").await;
    let token_client = McpClient::with_bearer(&server.base_url, &token);
    let me = token_client.tools_call("host.whoami", json!({})).await.expect("whoami by token");
    assert_eq!(me["structuredContent"]["scopes"]["effective"], json!(["read"]));

    // A catalog word is effective; a word in neither place never gets minted.
    key_client
        .tools_call("host.oauth.scope_set", json!({"name": "reports", "description": "Run reports"}))
        .await
        .expect("scope_set");
    let token = consent_and_get_token(&server, &ns, &key, "read reports").await;
    let me = McpClient::with_bearer(&server.base_url, &token)
        .tools_call("host.whoami", json!({}))
        .await
        .expect("whoami");
    assert_eq!(me["structuredContent"]["scopes"]["effective"], json!(["read", "reports"]));
}

#[test]
fn sreg_ac07_effective_for_drops_foreign_words_and_dedupes() {
    let catalog = vec!["reports".to_string()];
    let got = mcphost::scopes::effective_for(Some("read bogus read reports offline_access"), &catalog);
    assert_eq!(got, vec!["read".to_string(), "reports".to_string()]);
    assert_eq!(mcphost::scopes::effective_for(None, &catalog), vec!["mcp".to_string()]);
}
