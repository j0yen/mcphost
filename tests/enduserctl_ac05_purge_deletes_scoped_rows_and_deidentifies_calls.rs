//! PRD-mcphost-end-user-audit-and-revoke
//! AC5 (P0) -- Given u1 is revoked and owns 20 scoped state rows and 1
//! vault token, When `host.enduser.purge {subject: "u1"}` runs, Then the
//! rows and token are deleted, u1's `calls` rows remain with null
//! `end_user_subject`, `purged_at` is set, the audit tombstone exists, and
//! the response reports `{state_rows: 20, vault_tokens: 1}`.

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
async fn purge_deletes_scoped_rows_and_deidentifies_calls() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "AC5 Tenant").await;
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

    let tenant = server.state.db.find_tenant_by_namespace(ns).await.unwrap().expect("tenant");
    for _ in 0..20 {
        server
            .state
            .db
            .state_row_insert(tenant.id, "mytable".to_string(), "{}".to_string(), "u1".to_string())
            .await
            .expect("seed state row");
    }
    server
        .state
        .db
        .insert_vault_token_for_test(tenant.id, "u1".to_string(), "slack".to_string())
        .await
        .expect("seed vault token");

    client
        .tools_call("host.enduser.revoke", json!({"subject": "u1", "reason": "left"}))
        .await
        .expect("revoke ok");

    let purged = extract_structured(
        &client
            .tools_call("host.enduser.purge", json!({"subject": "u1"}))
            .await
            .expect("host.enduser.purge ok"),
    );
    assert_eq!(purged["state_rows"], json!(20), "{purged:?}");
    assert_eq!(purged["vault_tokens"], json!(1), "{purged:?}");

    let remaining_rows = server
        .state
        .db
        .state_rows_for_end_user(tenant.id, "mytable".to_string(), "u1".to_string())
        .await
        .expect("query remaining rows");
    assert!(remaining_rows.is_empty(), "{remaining_rows:?}");

    let remaining_vault = server
        .state
        .db
        .count_vault_connections_for_subject(tenant.id, "u1".to_string())
        .await
        .expect("count remaining vault connections");
    assert_eq!(remaining_vault, 0, "vault token must be deleted, not just revoked");

    let (subject_col, _issuer_col, _method_col) = server
        .state
        .db
        .last_call_end_user_for_test(tenant.id, "t1".to_string())
        .await
        .expect("query last call")
        .expect("a calls row must still exist for t1");
    assert_eq!(subject_col, None, "calls.end_user_subject must be null after purge");

    let got = extract_structured(
        &client
            .tools_call("host.enduser.get", json!({"subject": "u1"}))
            .await
            .expect("host.enduser.get ok"),
    );
    assert!(got["purged_at"].as_i64().is_some(), "{got:?}");

    let audited = extract_structured(
        &client
            .tools_call("host.enduser.audit", json!({"subject": "u1"}))
            .await
            .expect("host.enduser.audit ok"),
    );
    let has_purge_event = audited["entries"]
        .as_array()
        .unwrap()
        .iter()
        .any(|e| e["type"] == json!("event") && e["action"] == json!("purge"));
    assert!(has_purge_event, "audit must show the purge tombstone: {audited:?}");
}
