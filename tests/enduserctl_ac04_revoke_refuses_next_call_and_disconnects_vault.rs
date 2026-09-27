//! PRD-mcphost-end-user-audit-and-revoke
//! AC4 (P0) -- Given `host.enduser.revoke {subject: "u1", reason: "left"}`,
//! When u1's next identified call arrives, Then it returns
//! `end_user_revoked`, no tool runs, and u1's vault tokens are marked
//! revoked.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use serde::Serialize;
use serde_json::json;

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

fn sign_assertion(secret: &str, sub: &str) -> String {
    let now = now_unix();
    let claims = AssertionClaims { sub: sub.to_string(), iat: now, exp: now + 300 };
    encode(&Header::new(Algorithm::HS256), &claims, &EncodingKey::from_secret(secret.as_bytes()))
        .expect("sign assertion")
}

#[tokio::test]
async fn revoke_refuses_next_call_and_disconnects_vault() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "AC4 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "t1", "kind": "echo", "spec": {"schema": {"type": "object"}}}),
        )
        .await
        .expect("publish ok");

    let rotate = client
        .tools_call("host.enduser.assertion_secret_rotate", json!({}))
        .await
        .expect("assertion_secret_rotate ok");
    let secret = extract_structured(&rotate)["secret"].as_str().expect("secret string").to_string();

    let assertion = sign_assertion(&secret, "u1");
    client
        .tools_call(&format!("{ns}.t1"), json!({"end_user_assertion": assertion}))
        .await
        .unwrap_or_else(|e| panic!("u1's first call must succeed: {} {}", e.code, e.message));

    let tenant = server.state.db.find_tenant_by_namespace(ns.clone()).await.unwrap().expect("tenant");
    server
        .state
        .db
        .insert_vault_token_for_test(tenant.id, "u1".to_string(), "slack".to_string())
        .await
        .expect("seed vault token");

    let revoked = extract_structured(
        &client
            .tools_call("host.enduser.revoke", json!({"subject": "u1", "reason": "left"}))
            .await
            .expect("host.enduser.revoke ok"),
    );
    assert_eq!(revoked["revoked"], json!(true), "{revoked:?}");

    let assertion = sign_assertion(&secret, "u1");
    let err = client
        .tools_call(&format!("{ns}.t1"), json!({"end_user_assertion": assertion}))
        .await
        .expect_err("a revoked end user's next call must be refused");
    assert_eq!(err.error_code.as_deref(), Some("end_user_revoked"), "{err:?}");

    // no tool run: still exactly one `call` entry in the audit trail.
    let audited = extract_structured(
        &client
            .tools_call("host.enduser.audit", json!({"subject": "u1"}))
            .await
            .expect("host.enduser.audit ok"),
    );
    let call_entries: Vec<_> =
        audited["entries"].as_array().unwrap().iter().filter(|e| e["type"] == json!("call")).collect();
    assert_eq!(call_entries.len(), 1, "no new calls row after revoke: {call_entries:?}");

    let revoked_vault_tokens = server
        .state
        .db
        .count_revoked_vault_tokens_for_test(tenant.id, "u1".to_string())
        .await
        .expect("count revoked vault tokens");
    assert_eq!(revoked_vault_tokens, 1, "u1's vault token must be marked revoked");
}
