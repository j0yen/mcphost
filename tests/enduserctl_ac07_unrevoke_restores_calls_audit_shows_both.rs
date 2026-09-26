//! PRD-mcphost-end-user-audit-and-revoke
//! AC7 (P0) -- Given `unrevoke {subject: "u1"}` after revoke (before
//! purge), When u1 calls again, Then the call succeeds and the audit shows
//! revoke and unrevoke entries.

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
async fn unrevoke_restores_calls_and_audit_shows_both_entries() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "AC7 Tenant").await;
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

    client
        .tools_call("host.enduser.revoke", json!({"subject": "u1", "reason": "left"}))
        .await
        .expect("revoke ok");

    let assertion = sign_assertion(&secret, "u1");
    client
        .tools_call(&format!("{ns}.t1"), json!({"end_user_assertion": assertion}))
        .await
        .expect_err("still revoked, must be refused");

    let unrevoked = extract_structured(
        &client
            .tools_call("host.enduser.unrevoke", json!({"subject": "u1"}))
            .await
            .expect("host.enduser.unrevoke ok"),
    );
    assert_eq!(unrevoked["revoked"], json!(false), "{unrevoked:?}");

    let assertion = sign_assertion(&secret, "u1");
    client
        .tools_call(&format!("{ns}.t1"), json!({"end_user_assertion": assertion}))
        .await
        .unwrap_or_else(|e| panic!("call after unrevoke must succeed: {} {}", e.code, e.message));

    let audited = extract_structured(
        &client
            .tools_call("host.enduser.audit", json!({"subject": "u1"}))
            .await
            .expect("host.enduser.audit ok"),
    );
    let entries = audited["entries"].as_array().expect("entries array");
    let events: Vec<&str> = entries
        .iter()
        .filter(|e| e["type"] == json!("event"))
        .map(|e| e["action"].as_str().unwrap())
        .collect();
    assert!(events.contains(&"revoke"), "audit must show revoke: {events:?}");
    assert!(events.contains(&"unrevoke"), "audit must show unrevoke: {events:?}");

    let calls = entries.iter().filter(|e| e["type"] == json!("call")).count();
    assert_eq!(calls, 2, "the refused attempt must not have written a calls row: {entries:?}");
}
