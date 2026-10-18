//! PRD-mcphost-scope-registry-generated
//! AC5 (P1) — Given a key caller and an OAuth caller with scope `read`, When
//! each calls `host.whoami`, Then `scopes.all` is identical and
//! `scopes.effective` is `["mcp"]` for the key and `["read"]` for the token.

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
async fn sreg_ac05_whoami_reports_registry_and_effective_scopes() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "Whoami Scopes Tenant").await;
    let key_client = McpClient::with_bearer(&server.base_url, &key);
    let by_key = key_client.tools_call("host.whoami", json!({})).await.expect("whoami by key");

    let token = consent_and_get_token(&server, &ns, &key, "read").await;
    let token_client = McpClient::with_bearer(&server.base_url, &token);
    let by_token = token_client.tools_call("host.whoami", json!({})).await.expect("whoami by token");

    let key_scopes = &by_key["structuredContent"]["scopes"];
    let token_scopes = &by_token["structuredContent"]["scopes"];
    let expected_all: Vec<&str> = mcphost::scopes::Scope::ALL.iter().map(|s| s.as_str()).collect();
    assert_eq!(key_scopes["all"], json!(expected_all));
    assert_eq!(key_scopes["all"], token_scopes["all"]);
    assert_eq!(key_scopes["effective"], json!(["mcp"]));
    assert_eq!(token_scopes["effective"], json!(["read"]));
}
