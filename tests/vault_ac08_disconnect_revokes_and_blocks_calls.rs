//! PRD-mcphost-upstream-token-vault
//! AC8 (P0) — Given `host.vault.disconnect {provider: "slack", end_user:
//! "self"}`, When u1 calls the tool again, Then `upstream_not_connected`
//! returns and `vault_tokens.revoked_at` is set.

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
async fn disconnect_revokes_the_token_and_blocks_the_next_call() {
    let upstream = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ok": true})))
        .mount(&upstream)
        .await;

    let server = TestServer::start_with_kinds(http_kind_registry()).await;
    let (ns, key) = signup(&server.base_url, "AC8 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

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

    // Sanity: u1 is connected and the call succeeds before disconnecting.
    let assertion = sign_assertion(&client, "u1").await;
    let ok = client
        .tools_call(
            &format!("{ns}.slack_call"),
            json!({"end_user_assertion": assertion.clone()}),
        )
        .await
        .expect("call before disconnect must succeed");
    assert_eq!(extract_structured(&ok)["status"], 200);

    client
        .tools_call(
            "host.vault.disconnect",
            json!({"provider": "slack", "end_user": "self", "end_user_assertion": assertion.clone()}),
        )
        .await
        .expect("disconnect ok");

    let row = server
        .state
        .db
        .get_vault_token(tenant.id, "slack".to_string(), "u1".to_string())
        .await
        .expect("db read ok")
        .expect("row still exists");
    assert!(row.revoked_unix.is_some(), "vault_tokens.revoked_at must be set");

    let err = client
        .tools_call(&format!("{ns}.slack_call"), json!({"end_user_assertion": assertion}))
        .await
        .expect_err("a call after disconnect must be refused");
    assert_eq!(err.error_code.as_deref(), Some("upstream_not_connected"));
}
