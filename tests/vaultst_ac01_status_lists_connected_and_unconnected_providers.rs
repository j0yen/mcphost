//! PRD-mcphost-upstream-token-vault-status
//! AC1 (P0) — Given u1 connected to provider `slack` through the test
//! OAuth server and provider `github` registered but never connected, When
//! u1 calls `host.vault.status {end_user: "self"}`, Then `providers` lists
//! `slack` with `connected: true`, a numeric `expires_at`, the granted
//! `scopes`, `revoked_at: null`, and `github` with `connected: false`, and
//! no substring of any access or refresh token appears in the response.

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

fn query_param<'a>(url: &'a str, name: &str) -> &'a str {
    let query = url.split_once('?').map(|(_, q)| q).unwrap_or("");
    query
        .split('&')
        .find_map(|pair| pair.strip_prefix(&format!("{name}=")))
        .unwrap_or_else(|| panic!("no '{name}' param in {url}"))
}

#[tokio::test]
async fn status_lists_connected_slack_and_unconnected_github() {
    let oauth_server = MockServer::start().await;
    const ACCESS_TOKEN: &str = "xoxp-vaultst-ac01-access-token";
    const REFRESH_TOKEN: &str = "xoxp-vaultst-ac01-refresh-token";
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
    let (_ns, key) = signup(&server.base_url, "AC1 Tenant").await;
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
                "scopes": ["read", "write"],
            }),
        )
        .await
        .expect("provider_set slack ok");
    client
        .tools_call(
            "host.vault.provider_set",
            json!({
                "name": "github",
                "auth_url": "https://github.example.invalid/authorize",
                "token_url": "https://github.example.invalid/token",
                "client_id": "gh-cid",
                "client_secret": "gh-secret",
                "scopes": ["repo"],
            }),
        )
        .await
        .expect("provider_set github ok");

    let assertion = sign_assertion(&client, "u1").await;
    let result = client
        .tools_call(
            "host.vault.connect_link",
            json!({"provider": "slack", "end_user": "self", "end_user_assertion": assertion.clone()}),
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

    let callback_url = format!("{}/vault/callback?code=test-auth-code&state={}", server.base_url, oauth_state);
    let callback_resp = reqwest::get(&callback_url).await.expect("GET callback");
    assert_eq!(callback_resp.status(), reqwest::StatusCode::OK);

    let status_result = client
        .tools_call(
            "host.vault.status",
            json!({"end_user": "self", "end_user_assertion": assertion}),
        )
        .await
        .unwrap_or_else(|e| panic!("host.vault.status must succeed: {} {}", e.code, e.message));
    let structured = extract_structured(&status_result);
    let providers = structured["providers"].as_array().expect("providers array");
    assert_eq!(providers.len(), 2, "{providers:?}");

    let slack = providers.iter().find(|p| p["name"] == json!("slack")).expect("slack entry");
    assert_eq!(slack["connected"], json!(true), "{slack:?}");
    assert!(slack["expires_at"].as_i64().is_some(), "{slack:?}");
    assert_eq!(slack["scopes"], json!("read write"), "{slack:?}");
    assert_eq!(slack["revoked_at"], json!(null), "{slack:?}");

    let github = providers.iter().find(|p| p["name"] == json!("github")).expect("github entry");
    assert_eq!(github["connected"], json!(false), "{github:?}");

    let raw = structured.to_string();
    assert!(!raw.contains(ACCESS_TOKEN), "response must never carry the access token: {raw}");
    assert!(!raw.contains(REFRESH_TOKEN), "response must never carry the refresh token: {raw}");
}
