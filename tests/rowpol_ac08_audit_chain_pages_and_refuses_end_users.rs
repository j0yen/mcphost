//! PRD-mcphost-row-policy
//! AC8 (P0) — Given `host.audit.chain(subject: "alice", limit: 10)`, When
//! called by the tenant key, Then ten newest records for Alice return with
//! `returned_count` and `withheld_count`; When called with an end-user
//! credential, Then it is refused.

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
async fn tenant_key_pages_alices_chain_and_an_end_user_credential_is_refused() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "AC8 Tenant").await;
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
        .tools_call("host.policy.attrs_set", json!({"subject": "alice", "attrs": {"region": "EU"}}))
        .await
        .expect("attrs set ok");

    let assertion = sign_assertion(&secret, "alice");
    for _ in 0..15 {
        client
            .tools_call(
                "host.table.query",
                json!({"sql": "SELECT COUNT(*) AS n FROM orders", "end_user_assertion": assertion.clone()}),
            )
            .await
            .expect("filtered read must succeed");
    }

    let chain = client
        .tools_call("host.audit.chain", json!({"subject": "alice", "limit": 10}))
        .await
        .expect("audit chain ok (tenant key)");
    let chain = extract_structured(&chain);
    let records = chain["records"].as_array().expect("records array");
    assert_eq!(records.len(), 10, "{chain:?}");
    assert_eq!(chain["returned_count"], json!(10), "{chain:?}");
    assert_eq!(chain["withheld_count"], json!(5), "15 total minus the 10 returned: {chain:?}");
    for record in records {
        assert_eq!(record["subject"], json!("alice"), "{record:?}");
    }
    // Newest first: ids must be strictly descending.
    let ids: Vec<i64> = records.iter().map(|r| r["id"].as_i64().unwrap()).collect();
    let mut sorted_desc = ids.clone();
    sorted_desc.sort_by(|a, b| b.cmp(a));
    assert_eq!(ids, sorted_desc, "records must be newest first: {ids:?}");

    let refused = client
        .tools_call("host.audit.chain", json!({"subject": "alice", "limit": 10, "end_user_assertion": assertion}))
        .await
        .expect_err("an end-user credential must be refused, not silently scoped");
    assert_eq!(refused.error_code.as_deref(), Some("tenant_key_required"), "{refused:?}");
}
