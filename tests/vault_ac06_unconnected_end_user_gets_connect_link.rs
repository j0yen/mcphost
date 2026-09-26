//! PRD-mcphost-upstream-token-vault
//! AC6 (P0) — Given u2 who never connected, When u2 calls the same tool,
//! Then the call returns `upstream_not_connected` with a fresh
//! `connect_link` URL and no upstream request is made.

use crate::common;
use common::{McpClient, TestServer, extract_structured, http_kind_registry, signup};
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use serde::Serialize;
use serde_json::json;
use wiremock::matchers::method;
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
async fn u2_never_connected_gets_a_fresh_connect_link_and_no_upstream_call() {
    let upstream = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ok": true})))
        .mount(&upstream)
        .await;

    let server = TestServer::start_with_kinds(http_kind_registry()).await;
    let (ns, key) = signup(&server.base_url, "AC6 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call(
            "host.vault.provider_set",
            json!({
                "name": "slack",
                "auth_url": "https://slack.example.invalid/authorize",
                "token_url": "https://slack.example.invalid/token",
                "client_id": "cid-123",
                "client_secret": "shh-secret",
                "scopes": ["read"],
            }),
        )
        .await
        .expect("provider_set ok");

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

    // AC1's precedent: u1 connects.
    let tenant = server
        .state
        .db
        .find_tenant_by_namespace(ns.clone())
        .await
        .unwrap()
        .expect("tenant");
    let (access_enc, access_nonce) = server.state.secrets.encrypt("u1-token").expect("encrypt");
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
            None,
            None,
            now_unix() + 3600,
            "read".to_string(),
        )
        .await
        .expect("seed vault token for u1");

    // u2 never connected.
    let assertion = sign_assertion(&client, "u2").await;
    let err = client
        .tools_call(
            &format!("{ns}.slack_call"),
            json!({"end_user_assertion": assertion}),
        )
        .await
        .expect_err("u2's call must be refused");
    assert_eq!(err.error_code.as_deref(), Some("upstream_not_connected"));
    let connect_link = err.data["connect_link"].as_str().expect("connect_link in error data");
    assert!(
        connect_link.starts_with(&format!("{}/vault/connect/", server.base_url)),
        "connect_link: {connect_link}"
    );

    assert!(
        upstream.received_requests().await.unwrap().is_empty(),
        "no upstream request may be made for an unconnected end user"
    );
}
