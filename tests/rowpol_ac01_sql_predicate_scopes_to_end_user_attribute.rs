//! PRD-mcphost-row-policy
//! AC1 (P0) — Given `orders` with a policy `region = Attr(region)` and
//! Alice with `region = EU`, When Alice runs `SELECT COUNT(*) FROM
//! orders`, Then the count equals the EU rows only and the audit record's
//! `policy_hash` equals the compiled hash for Alice.

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
async fn alice_sees_only_eu_rows_and_the_audit_record_carries_her_compiled_hash() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "AC1 Tenant").await;
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
        .tools_call(
            "host.table.append",
            json!({"table": "orders", "rows": [
                {"id": 1, "region": "EU"},
                {"id": 2, "region": "EU"},
                {"id": 3, "region": "EU"},
                {"id": 4, "region": "US"},
                {"id": 5, "region": "US"},
            ]}),
        )
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
        .tools_call("host.policy.attrs_set", json!({"subject": "alice", "attrs": {"region": "EU"}}))
        .await
        .expect("attrs set ok");

    let assertion = sign_assertion(&secret, "alice");
    let result = client
        .tools_call(
            "host.table.query",
            json!({"sql": "SELECT COUNT(*) AS n FROM orders", "end_user_assertion": assertion}),
        )
        .await
        .unwrap_or_else(|e| panic!("alice's query must succeed: {} {}", e.code, e.message));
    let structured = extract_structured(&result);
    let rows = structured["rows"].as_array().expect("rows array");
    assert_eq!(rows.len(), 1, "{structured}");
    assert_eq!(rows[0]["n"], json!(3), "Alice must see only the 3 EU rows: {structured}");

    let tenant = server
        .state
        .db
        .find_tenant_by_namespace(ns)
        .await
        .expect("db ok")
        .expect("tenant exists");

    let (records, _total) = server
        .state
        .db
        .audit_chain_list(tenant.id, Some("alice".to_string()), 1, None)
        .await
        .expect("audit chain list ok");
    let record = records.first().expect("an audit record must exist for alice's filtered read");
    assert_eq!(record.plane, "sql");

    let policy = mcphost::rowpolicy::load_table_policy(&server.state, tenant.id, "orders")
        .await
        .expect("load policy ok");
    let attrs = server
        .state
        .db
        .end_user_attrs_get(tenant.id, "alice".to_string())
        .await
        .expect("attrs get ok");
    let ctx = mcphost::rowpolicy::SecurityContext {
        subject: "alice".to_string(),
        issuer: None,
        method: "assertion".to_string(),
        attrs,
    };
    let compiled = mcphost::rowpolicy::policy::compile(&policy, &ctx);
    assert_eq!(
        record.policy_hash, compiled.policy_hash,
        "the audit record's policy_hash must equal the compiled hash for Alice"
    );
}
