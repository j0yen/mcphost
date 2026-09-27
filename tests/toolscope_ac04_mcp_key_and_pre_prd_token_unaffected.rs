//! PRD-mcphost-tool-scopes-and-consent
//! AC4 (P0) — Given a token with `scope=mcp`, a tenant key, and a token
//! issued before this PRD, When each calls `delete_records`, Then all
//! three succeed.
//!
//! "A token issued before this PRD" is a bring-your-own-issuer bearer JWT
//! carrying no `scope` claim at all -- the only shape a pre-PRD token
//! could be (this PRD is what introduced fine-grained `scope` values in
//! the first place; every pre-existing OAuth test's tokens never set one
//! either, `oauth::OauthCaller::scope: None`, requirement 5).

use crate::common;
use common::{McpClient, TestServer, signup};
use serde_json::{Value, json};

use crate::oauth;
use oauth::{KID_1, jwk_1, jwks_server, priv_pem_1, sign};

async fn setup(server: &TestServer) -> (String, String) {
    let (ns, key) = signup(&server.base_url, "Preexisting Access Tenant").await;
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
        .expect("publish search");
    key_client
        .tools_call(
            "host.tool_publish",
            json!({"name": "delete_records", "kind": "echo", "spec": {"schema": {"type": "object"}}, "scopes": ["write"]}),
        )
        .await
        .expect("publish delete_records");
    (ns, key)
}

#[tokio::test]
async fn mcp_scoped_token_key_and_preexisting_oauth_token_all_call_delete_records() {
    let server = TestServer::start().await;
    let (ns, key) = setup(&server).await;

    // 1. Tenant key: full access, unaffected (Non-goals: "scopes for
    // key-based calls").
    let key_client = McpClient::with_bearer(&server.base_url, &key);
    key_client
        .tools_call(&format!("{ns}.delete_records"), json!({}))
        .await
        .expect("a tenant key must always reach delete_records");

    // 2. A hosted token with scope=mcp: the hierarchy root, satisfies
    // every scope.
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
    let verifier = "a-pkce-verifier-at-least-43-chars-long-for-realism-ac4";
    let challenge = {
        use base64::Engine as _;
        use sha2::{Digest, Sha256};
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
    };
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
            ("scope", "mcp"),
            ("resource", resource.as_str()),
            ("tenant_key", key.as_str()),
        ])
        .send()
        .await
        .expect("POST /oauth/authorize");
    let location = consent_resp.headers().get("location").expect("Location header").to_str().unwrap().to_string();
    let (_, query) = location.split_once('?').expect("redirect must carry a query string");
    let code = query
        .split('&')
        .find_map(|p| p.split_once('=').filter(|(k, _)| *k == "code").map(|(_, v)| v))
        .expect("code present in redirect")
        .to_string();
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
    assert_eq!(token_resp["scope"], json!("mcp"));
    let mcp_token = token_resp["access_token"].as_str().expect("access_token").to_string();
    let mcp_client = McpClient::with_bearer(&server.base_url, &mcp_token);
    mcp_client
        .tools_call(&format!("{ns}.delete_records"), json!({}))
        .await
        .expect("a scope=mcp hosted token must reach delete_records");

    // 3. A bring-your-own-issuer JWT with no `scope` claim at all --
    // this PRD's own requirement 5 keeps it unrestricted.
    let jwks = jwks_server(jwk_1()).await;
    let issuer = "https://issuer.toolscope-ac4.example.com".to_string();
    key_client
        .tools_call(
            "host.oauth.issuer_set",
            json!({"issuer": issuer, "jwks_url": format!("{}/jwks", jwks.uri())}),
        )
        .await
        .expect("issuer_set must succeed");
    std::mem::forget(jwks);
    let resource = format!("{}/t/{ns}/mcp", server.base_url);
    let byoi_token = sign(KID_1, priv_pem_1(), &issuer, &resource, "preexisting-user", 300);
    let byoi_client = McpClient::with_bearer(&server.base_url, &byoi_token).with_path(&format!("/t/{ns}/mcp"));
    byoi_client
        .tools_call(&format!("{ns}.delete_records"), json!({}))
        .await
        .expect("a pre-PRD (no scope claim) token must reach delete_records");
}
