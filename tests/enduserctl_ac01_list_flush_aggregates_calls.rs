//! PRD-mcphost-end-user-audit-and-revoke
//! AC1 (P0) -- Given 3 identified calls from u1 and 1 from u2, When the 10s
//! flush runs and `host.enduser.list` is called, Then two rows return with
//! `calls_total` 3 and 1 and correct `last_seen`.

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
async fn list_after_flush_aggregates_calls_per_subject() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "AC1 Tenant").await;
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

    for _ in 0..3 {
        let assertion = sign_assertion(&secret, "u1");
        client
            .tools_call(&format!("{ns}.t1"), json!({"end_user_assertion": assertion}))
            .await
            .unwrap_or_else(|e| panic!("u1 call must succeed: {} {}", e.code, e.message));
    }
    let assertion = sign_assertion(&secret, "u2");
    client
        .tools_call(&format!("{ns}.t1"), json!({"end_user_assertion": assertion}))
        .await
        .unwrap_or_else(|e| panic!("u2 call must succeed: {} {}", e.code, e.message));

    mcphost::enduserctl::flush_once(&server.state).await.expect("flush_once ok");

    let listed = extract_structured(
        &client.tools_call("host.enduser.list", json!({})).await.expect("host.enduser.list ok"),
    );
    let rows = listed["end_users"].as_array().expect("end_users array");
    assert_eq!(rows.len(), 2, "rows: {rows:?}");

    let u1 = rows.iter().find(|r| r["subject"] == json!("u1")).expect("u1 row present");
    assert_eq!(u1["calls_total"], json!(3), "{u1:?}");
    assert!(u1["last_seen"].as_i64().unwrap() > 0, "{u1:?}");

    let u2 = rows.iter().find(|r| r["subject"] == json!("u2")).expect("u2 row present");
    assert_eq!(u2["calls_total"], json!(1), "{u2:?}");
    assert!(u2["last_seen"].as_i64().unwrap() > 0, "{u2:?}");
}
