//! PRD-mcphost-end-user-audit-and-revoke
//! AC2 (P0) -- Given u1, When `host.enduser.get {subject: "u1"}` runs, Then
//! it returns the row plus `state_rows`, `vault_connections`, and
//! `runs_30d` counts matching the tables.

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
async fn get_returns_row_plus_live_counts() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "AC2 Tenant").await;
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

    for _ in 0..2 {
        let assertion = sign_assertion(&secret, "u1");
        client
            .tools_call(&format!("{ns}.t1"), json!({"end_user_assertion": assertion}))
            .await
            .unwrap_or_else(|e| panic!("u1 call must succeed: {} {}", e.code, e.message));
    }
    mcphost::enduserctl::flush_once(&server.state).await.expect("flush_once ok");

    // One tenant_state_kv row for u1 via the real API...
    let assertion = sign_assertion(&secret, "u1");
    client
        .tools_call(
            "host.state.set",
            json!({"key": "k1", "value": 1, "end_user": "self", "end_user_assertion": assertion}),
        )
        .await
        .unwrap_or_else(|e| panic!("state.set must succeed: {} {}", e.code, e.message));

    // ...and two tenant_state_rows rows, seeded directly (no `host.table.*`
    // ceremony needed just to prove the count).
    let tenant = server.state.db.find_tenant_by_namespace(ns).await.unwrap().expect("tenant");
    server
        .state
        .db
        .state_row_insert(tenant.id, "mytable".to_string(), "{}".to_string(), "u1".to_string())
        .await
        .expect("seed row 1");
    server
        .state
        .db
        .state_row_insert(tenant.id, "mytable".to_string(), "{}".to_string(), "u1".to_string())
        .await
        .expect("seed row 2");

    server
        .state
        .db
        .insert_vault_token_for_test(tenant.id, "u1".to_string(), "slack".to_string())
        .await
        .expect("seed vault token");

    let got = extract_structured(
        &client
            .tools_call("host.enduser.get", json!({"subject": "u1"}))
            .await
            .expect("host.enduser.get ok"),
    );

    assert_eq!(got["subject"], json!("u1"), "{got:?}");
    assert_eq!(got["calls_total"], json!(2), "{got:?}");
    assert_eq!(got["state_rows"], json!(3), "{got:?} (2 tenant_state_rows + 1 tenant_state_kv)");
    assert_eq!(got["vault_connections"], json!(1), "{got:?}");
    assert_eq!(got["runs_30d"], json!(2), "{got:?}");
}
