//! PRD-mcphost-upstream-token-vault
//! AC2 (P0) — Given the test server redirects back with a code, When
//! `/vault/callback` runs, Then the code is exchanged server-side,
//! `vault_tokens` holds encrypted access and refresh tokens for u1, and the
//! page shows the connected message with no token text.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use serde::Serialize;
use serde_json::json;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

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

/// Pulls one query parameter's raw value out of a URL string -- every value
/// this test needs (`state`) is a hex token with no characters that would
/// need percent-decoding.
fn query_param<'a>(url: &'a str, name: &str) -> &'a str {
    let query = url.split_once('?').map(|(_, q)| q).unwrap_or("");
    query
        .split('&')
        .find_map(|pair| pair.strip_prefix(&format!("{name}=")))
        .unwrap_or_else(|| panic!("no '{name}' param in {url}"))
}

#[tokio::test]
async fn callback_exchanges_code_and_stores_encrypted_tokens() {
    let oauth_server = MockServer::start().await;
    const ACCESS_TOKEN: &str = "xoxp-test-access-token-abc123";
    const REFRESH_TOKEN: &str = "xoxp-test-refresh-token-def456";
    Mock::given(method("POST"))
        .and(path("/token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "access_token": ACCESS_TOKEN,
            "refresh_token": REFRESH_TOKEN,
            "expires_in": 3600,
        })))
        .mount(&oauth_server)
        .await;

    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "AC2 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call(
            "host.vault.provider_set",
            json!({
                "name": "slack",
                "auth_url": format!("{}/authorize", oauth_server.uri()),
                "token_url": format!("{}/token", oauth_server.uri()),
                "client_id": "cid-123",
                "client_secret": "shh-secret",
                "scopes": ["read"],
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
        .expect("connect_link ok");
    let connect_link = extract_structured(&result)["connect_link"]
        .as_str()
        .expect("connect_link string")
        .to_string();

    let no_redirect = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("build no-redirect client");
    let redirect_resp = no_redirect.get(&connect_link).send().await.expect("GET connect_link");
    let location = redirect_resp
        .headers()
        .get(reqwest::header::LOCATION)
        .expect("Location header")
        .to_str()
        .unwrap()
        .to_string();
    let oauth_state = query_param(&location, "state").to_string();

    let callback_url = format!(
        "{}/vault/callback?code=test-auth-code-789&state={}",
        server.base_url, oauth_state
    );
    let callback_resp = reqwest::get(&callback_url).await.expect("GET callback");
    assert_eq!(callback_resp.status(), reqwest::StatusCode::OK);
    let body = callback_resp.text().await.expect("body");
    assert!(body.contains("Connected"), "body: {body}");
    assert!(!body.contains(ACCESS_TOKEN), "body must never echo the access token: {body}");
    assert!(!body.contains(REFRESH_TOKEN), "body must never echo the refresh token: {body}");

    let tenant = server
        .state
        .db
        .find_tenant_by_namespace(ns)
        .await
        .unwrap()
        .expect("tenant");
    let row = server
        .state
        .db
        .get_vault_token(tenant.id, "slack".to_string(), "u1".to_string())
        .await
        .expect("db read ok")
        .expect("a vault_tokens row must exist for u1");
    assert_ne!(row.access_enc, ACCESS_TOKEN.as_bytes(), "access token must not be stored in plaintext");
    let decrypted_access = server.state.secrets.decrypt(&row.access_enc, &row.access_nonce).expect("decrypt access");
    assert_eq!(decrypted_access, ACCESS_TOKEN);
    let refresh_enc = row.refresh_enc.expect("refresh_enc must be set");
    let refresh_nonce = row.refresh_nonce.expect("refresh_nonce must be set");
    assert_ne!(refresh_enc, REFRESH_TOKEN.as_bytes(), "refresh token must not be stored in plaintext");
    let decrypted_refresh = server.state.secrets.decrypt(&refresh_enc, &refresh_nonce).expect("decrypt refresh");
    assert_eq!(decrypted_refresh, REFRESH_TOKEN);
    assert!(row.revoked_unix.is_none(), "a freshly connected token must not be revoked");
}
