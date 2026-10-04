//! PRD-mcphost-row-policy
//! AC7 (P0) — Given 50 filtered reads, When `host.audit.verify` runs over
//! them, Then `intact` is true; When one record's `applied` field is
//! altered in the database, Then `intact` is false and the first broken id
//! is reported.

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
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64
}

fn sign_assertion(secret: &str, sub: &str) -> String {
    let now = now_unix();
    let claims = AssertionClaims { sub: sub.to_string(), iat: now, exp: now + 300 };
    encode(&Header::new(Algorithm::HS256), &claims, &EncodingKey::from_secret(secret.as_bytes()))
        .expect("sign test assertion")
}

#[tokio::test]
async fn fifty_filtered_reads_verify_intact_then_a_tampered_record_breaks_verification() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "AC7 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let rotate = client
        .tools_call("host.enduser.assertion_secret_rotate", json!({}))
        .await
        .expect("assertion_secret_rotate ok");
    let secret = extract_structured(&rotate)["secret"].as_str().expect("secret string").to_string();

    client
        .tools_call(
            "host.table.create",
            json!({"name": "orders", "columns": {"id": "integer", "region": "text"}, "primary_key": "id"}),
        )
        .await
        .expect("table create ok");
    client
        .tools_call("host.table.append", json!({"table": "orders", "rows": [{"id": 1, "region": "EU"}]}))
        .await
        .expect("append ok");
    client
        .tools_call(
            "host.policy.set",
            json!({
                "target": {"table": "orders"},
                "rule": [{"column_or_attr": "region", "op": "eq", "value": {"attr": "region"}}],
            }),
        )
        .await
        .expect("policy set ok");
    client
        .tools_call("host.policy.attrs_set", json!({"subject": "erin", "attrs": {"region": "EU"}}))
        .await
        .expect("attrs set ok");

    let assertion = sign_assertion(&secret, "erin");
    for _ in 0..50 {
        client
            .tools_call(
                "host.table.query",
                json!({"sql": "SELECT COUNT(*) AS n FROM orders", "end_user_assertion": assertion.clone()}),
            )
            .await
            .expect("filtered read must succeed");
    }

    let tenant = server
        .state
        .db
        .find_tenant_by_namespace(ns)
        .await
        .expect("db ok")
        .expect("tenant exists");
    let (records, total) = server
        .state
        .db
        .audit_chain_list(tenant.id, Some("erin".to_string()), 1000, None)
        .await
        .expect("audit chain list ok");
    assert_eq!(total, 50, "exactly 50 audit records must exist for erin's 50 filtered reads");
    let min_id = records.iter().map(|r| r.id).min().expect("at least one record");
    let max_id = records.iter().map(|r| r.id).max().expect("at least one record");

    let verify_before = client
        .tools_call("host.audit.verify", json!({"from_id": min_id, "to_id": max_id}))
        .await
        .expect("audit verify ok");
    let verify_before = extract_structured(&verify_before);
    assert_eq!(verify_before["intact"], json!(true), "{verify_before:?}");
    assert_eq!(verify_before["first_broken_id"], json!(null), "{verify_before:?}");

    // Alter one record's `applied` field directly in the database.
    let mut sorted_ids: Vec<i64> = records.iter().map(|r| r.id).collect();
    sorted_ids.sort();
    let tampered_id = sorted_ids[sorted_ids.len() / 2];
    server
        .state
        .db
        .test_tamper_audit_applied(tampered_id, "1 = 1".to_string())
        .await
        .expect("tamper ok");

    let verify_after = client
        .tools_call("host.audit.verify", json!({"from_id": min_id, "to_id": max_id}))
        .await
        .expect("audit verify ok");
    let verify_after = extract_structured(&verify_after);
    assert_eq!(verify_after["intact"], json!(false), "{verify_after:?}");
    assert_eq!(
        verify_after["first_broken_id"],
        json!(tampered_id),
        "the first broken id must be the tampered record's own id: {verify_after:?}"
    );
}
