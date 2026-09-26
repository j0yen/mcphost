//! PRD-mcphost-upstream-token-vault
//! AC1 (P0) — Given a tenant registered provider `slack` against a test
//! OAuth server, When end user u1 calls `host.vault.connect_link`, Then a
//! URL under `/vault/connect/` returns, and `GET` on it redirects to the
//! test server's auth URL with `code_challenge_method=S256` and the
//! registered scopes.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use serde::Serialize;
use serde_json::json;
use wiremock::MockServer;

#[derive(Serialize)]
struct AssertionClaims {
    sub: String,
    iat: i64,
    exp: i64,
}

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

async fn sign_assertion(client: &McpClient, subject: &str) -> String {
    let rotate = client
        .tools_call("host.enduser.assertion_secret_rotate", json!({}))
        .await
        .expect("assertion_secret_rotate ok");
    let secret = extract_structured(&rotate)["secret"]
        .as_str()
        .expect("secret string")
        .to_string();
    let now = now_unix();
    let claims = AssertionClaims { sub: subject.to_string(), iat: now, exp: now + 300 };
    encode(
        &Header::new(Algorithm::HS256),
        &claims,
        &EncodingKey::from_secret(secret.as_bytes()),
    )
    .expect("sign assertion")
}

#[tokio::test]
async fn connect_link_redirects_with_pkce_and_scopes() {
    let oauth_server = MockServer::start().await;
    let auth_url = format!("{}/authorize", oauth_server.uri());
    let token_url = format!("{}/token", oauth_server.uri());

    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "AC1 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call(
            "host.vault.provider_set",
            json!({
                "name": "slack",
                "auth_url": auth_url,
                "token_url": token_url,
                "client_id": "cid-123",
                "client_secret": "shh-secret",
                "scopes": ["read", "write"],
            }),
        )
        .await
        .expect("provider_set ok");

    let assertion = sign_assertion(&client, "u1").await;
    let result = client
        .tools_call(
            "host.vault.connect_link",
            json!({"provider": "slack", "end_user": "self", "end_user_assertion": assertion}),
        )
        .await
        .unwrap_or_else(|e| panic!("connect_link must succeed: {} {}", e.code, e.message));
    let structured = extract_structured(&result);
    let connect_link = structured["connect_link"].as_str().expect("connect_link string").to_string();
    assert!(
        connect_link.starts_with(&format!("{}/vault/connect/", server.base_url)),
        "connect_link: {connect_link}"
    );

    let http = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("build no-redirect client");
    let resp = http.get(&connect_link).send().await.expect("GET connect_link");
    assert_eq!(resp.status(), reqwest::StatusCode::FOUND, "must redirect");
    let location = resp
        .headers()
        .get(reqwest::header::LOCATION)
        .expect("Location header")
        .to_str()
        .expect("Location is ascii")
        .to_string();

    assert!(location.starts_with(&auth_url), "location: {location}");
    assert!(location.contains("code_challenge_method=S256"), "location: {location}");
    assert!(location.contains("code_challenge="), "location: {location}");
    assert!(location.contains("scope=read%20write"), "location: {location}");
    assert!(location.contains("client_id=cid-123"), "location: {location}");
    assert!(location.contains("state="), "location: {location}");
}
