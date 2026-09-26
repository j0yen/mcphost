//! PRD-mcphost-upstream-token-vault-status
//! AC3 (P0) — Given u1's slack token revoked by a 401 on refresh, When u1
//! calls `host.vault.status`, Then `slack` shows `connected: false`, a
//! numeric `revoked_at`, and `revoked_reason` beginning `refresh_`.

use crate::common;
use common::{McpClient, TestServer, extract_structured, http_kind_registry, signup};
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

#[tokio::test]
async fn status_reports_revoked_after_a_401_refresh() {
    let oauth_server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/token"))
        .respond_with(ResponseTemplate::new(401).set_body_json(json!({"error": "invalid_grant"})))
        .mount(&oauth_server)
        .await;

    let upstream = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ok": true})))
        .mount(&upstream)
        .await;

    let server = TestServer::start_with_kinds(http_kind_registry()).await;
    let (ns, key) = signup(&server.base_url, "AC3 Tenant").await;
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
    let (refresh_enc, refresh_nonce) = server.state.secrets.encrypt("old-refresh-token").expect("encrypt");
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
    client
        .tools_call(&format!("{ns}.slack_call"), json!({"end_user_assertion": assertion.clone()}))
        .await
        .expect_err("the refresh-401 call must be refused");

    let status_result = client
        .tools_call(
            "host.vault.status",
            json!({"end_user": "self", "end_user_assertion": assertion}),
        )
        .await
        .unwrap_or_else(|e| panic!("host.vault.status must succeed: {} {}", e.code, e.message));
    let structured = extract_structured(&status_result);
    let providers = structured["providers"].as_array().expect("providers array");
    let slack = providers.iter().find(|p| p["name"] == json!("slack")).expect("slack entry");
    assert_eq!(slack["connected"], json!(false), "{slack:?}");
    assert!(slack["revoked_at"].as_i64().is_some(), "{slack:?}");
    let reason = slack["revoked_reason"].as_str().expect("revoked_reason string");
    assert!(reason.starts_with("refresh_"), "revoked_reason: {reason}");
}
