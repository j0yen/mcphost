//! PRD-mcphost-end-user-audit-and-revoke
//! AC6 (P0) -- Given u1 is not revoked, When `purge` runs, Then it is
//! rejected with `revoke_required`.

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
async fn purge_without_revoke_is_rejected() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "AC6 Tenant").await;
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
        .unwrap_or_else(|e| panic!("u1's call must succeed: {} {}", e.code, e.message));
    mcphost::enduserctl::flush_once(&server.state).await.expect("flush_once ok");

    // u1 is known (a real end_users row exists) but never revoked.
    let err = client
        .tools_call("host.enduser.purge", json!({"subject": "u1"}))
        .await
        .expect_err("purge on a non-revoked end user must be rejected");
    assert_eq!(err.error_code.as_deref(), Some("revoke_required"), "{err:?}");
}
