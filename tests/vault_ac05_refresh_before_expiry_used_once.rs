//! PRD-mcphost-upstream-token-vault
//! AC5 (P0) — Given a token expiring in 60 s and a valid refresh token,
//! When a call runs, Then the host refreshes first, stores the new token,
//! and the call uses it; the provider sees exactly one refresh request.

use crate::common;
use common::{McpClient, TestServer, extract_structured, http_kind_registry, signup};
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use serde::Serialize;
use serde_json::json;
use wiremock::matchers::{header, method, path};
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

#[tokio::test]
async fn expiring_token_is_refreshed_inline_exactly_once() {
    const OLD_REFRESH: &str = "old-refresh-token";
    const NEW_ACCESS: &str = "brand-new-access-token";
    const NEW_REFRESH: &str = "brand-new-refresh-token";

    let oauth_server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "access_token": NEW_ACCESS,
            "refresh_token": NEW_REFRESH,
            "expires_in": 3600,
        })))
        .mount(&oauth_server)
        .await;

    let upstream = MockServer::start().await;
    Mock::given(method("GET"))
        .and(header("Authorization", format!("Bearer {NEW_ACCESS}").as_str()))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ok": true})))
        .mount(&upstream)
        .await;

    let server = TestServer::start_with_kinds(http_kind_registry()).await;
    let (ns, key) = signup(&server.base_url, "AC5 Tenant").await;
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

    let tenant = server
        .state
        .db
        .find_tenant_by_namespace(ns.clone())
        .await
        .unwrap()
        .expect("tenant");
    let (access_enc, access_nonce) = server.state.secrets.encrypt("about-to-expire-token").expect("encrypt");
    let (refresh_enc, refresh_nonce) = server.state.secrets.encrypt(OLD_REFRESH).expect("encrypt");
    server
        .state
        .db
        .upsert_vault_token(
            tenant.id,
            "slack".to_string(),
            "u1".to_string(),
            None,
            access_enc,
            access_nonce,
            Some(refresh_enc),
            Some(refresh_nonce),
            now_unix() + 60,
            "read".to_string(),
        )
        .await
        .expect("seed vault token");

    let spec = json!({
        "method": "GET",
        "url": format!("{}/whoami", upstream.uri()),
        "upstream_provider": "slack",
        "args_schema": {"type": "object"},
    });
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "slack_call", "kind": "http", "spec": spec}),
        )
        .await
        .expect("publish ok");

    let assertion = sign_assertion(&client, "u1").await;
    let result = client
        .tools_call(
            &format!("{ns}.slack_call"),
            json!({"end_user_assertion": assertion}),
        )
        .await
        .unwrap_or_else(|e| panic!("call must succeed: {} {}", e.code, e.message));
    assert_eq!(extract_structured(&result)["status"], 200);

    let refresh_requests = oauth_server.received_requests().await.expect("mock records requests");
    assert_eq!(refresh_requests.len(), 1, "the provider must see exactly one refresh request");

    let row = server
        .state
        .db
        .get_vault_token(tenant.id, "slack".to_string(), "u1".to_string())
        .await
        .expect("db read ok")
        .expect("row still exists");
    let decrypted_access = server.state.secrets.decrypt(&row.access_enc, &row.access_nonce).expect("decrypt");
    assert_eq!(decrypted_access, NEW_ACCESS, "the stored token must be the refreshed one");
    assert!(row.last_refreshed_unix.is_some());
}
