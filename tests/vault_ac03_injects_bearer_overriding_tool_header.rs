//! PRD-mcphost-upstream-token-vault
//! AC3 (P0) — Given a connected u1, When u1 calls an http-kind tool with
//! `upstream_provider: "slack"`, Then the outbound request carries
//! `Authorization: Bearer <u1 token>` and the tool's own `Authorization`
//! header, if any, is replaced.

use crate::common;
use common::{McpClient, TestServer, extract_structured, http_kind_registry, signup};
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use serde::Serialize;
use serde_json::json;
use wiremock::matchers::{header, method};
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
async fn connected_u1_calls_get_the_vault_token_replacing_the_tools_own_header() {
    const VAULT_TOKEN: &str = "u1-vault-access-token-xyz";

    let upstream = MockServer::start().await;
    Mock::given(method("GET"))
        .and(header("Authorization", format!("Bearer {VAULT_TOKEN}").as_str()))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ok": true})))
        .mount(&upstream)
        .await;

    let server = TestServer::start_with_kinds(http_kind_registry()).await;
    let (ns, key) = signup(&server.base_url, "AC3 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let tenant = server
        .state
        .db
        .find_tenant_by_namespace(ns.clone())
        .await
        .unwrap()
        .expect("tenant");
    let (access_enc, access_nonce) = server.state.secrets.encrypt(VAULT_TOKEN).expect("encrypt");
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
        "headers": {"Authorization": "Bearer tool-declared-token-should-be-replaced"},
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
    let structured = extract_structured(&result);
    assert_eq!(structured["status"], 200, "{structured}");
}
