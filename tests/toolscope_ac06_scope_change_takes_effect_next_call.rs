//! PRD-mcphost-tool-scopes-and-consent
//! AC6 (P0) — Given T changes `search` to `scopes: ["write"]`, When the
//! `read`-scoped token calls `search`, Then 403 on the next call with no
//! token reissue.

use crate::common;
use common::{McpClient, TestServer, signup};
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

#[tokio::test]
async fn republishing_search_as_write_scoped_403s_the_same_token_with_no_reissue() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "Rescope Tenant").await;
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
    key_client
        .tools_call(
            "host.tool_publish",
            json!({"name": "search", "kind": "echo", "spec": {"schema": {"type": "object"}}, "scopes": ["read"]}),
        )
        .await
        .expect("publish search as read-scoped");

    // Mint a read-scoped token.
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
    let verifier = "ac6-verifier-at-least-43-characters-long-for-pkce";
    let challenge = code_challenge_for(verifier);
    let resource = mcphost::oauth::canonical_resource_uri(&server.base_url, &ns);
    let consent_resp = http
        .post(format!("{}/oauth/authorize", server.base_url))
        .form(&[
            ("response_type", "code"),
            ("client_id", client_id.as_str()),
            ("redirect_uri", "http://127.0.0.1/cb"),
            ("code_challenge", challenge.as_str()),
            ("code_challenge_method", "S256"),
            ("state", "abc"),
            ("scope", "read"),
            ("resource", resource.as_str()),
            ("tenant_key", key.as_str()),
        ])
        .send()
        .await
        .expect("POST /oauth/authorize");
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
            ("code_verifier", verifier),
        ])
        .send()
        .await
        .expect("POST /oauth/token")
        .json()
        .await
        .expect("parse token response");
    let access_token = token_resp["access_token"].as_str().expect("access_token").to_string();
    let token_client = McpClient::with_bearer(&server.base_url, &access_token);

    token_client
        .tools_call(&format!("{ns}.search"), json!({}))
        .await
        .expect("read-scoped token must reach read-scoped search before the republish");

    // T changes search to require write instead -- no new token minted.
    key_client
        .tools_call(
            "host.tool_publish",
            json!({"name": "search", "kind": "echo", "spec": {"schema": {"type": "object"}}, "scopes": ["write"]}),
        )
        .await
        .expect("republish search as write-scoped");

    let err = token_client
        .tools_call(&format!("{ns}.search"), json!({}))
        .await
        .expect_err("the same still-read-scoped token must now be refused with no reissue");
    assert_eq!(err.error_code.as_deref(), Some("insufficient_scope"));
    assert_eq!(err.data["tool"], json!("search"));
}
