//! PRD-mcphost-row-policy
//! AC2 (P0) — Given `orders` with a policy `region = Attr(region)` and Bob
//! with no `region` attribute, When Bob queries it, Then zero rows return
//! and the applied predicate is `1 = 0`.

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
async fn bob_with_no_region_attribute_sees_zero_rows_via_a_1_equals_0_predicate() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "AC2 Tenant").await;
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
                {"id": 2, "region": "US"},
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
    // Bob deliberately never gets host.policy.attrs_set -- no region
    // attribute exists for him at all.

    let assertion = sign_assertion(&secret, "bob");
    let result = client
        .tools_call(
            "host.table.query",
            json!({"sql": "SELECT COUNT(*) AS n FROM orders", "end_user_assertion": assertion}),
        )
        .await
        .unwrap_or_else(|e| panic!("bob's query must still succeed (fail closed, not refused): {} {}", e.code, e.message));
    let structured = extract_structured(&result);
    let rows = structured["rows"].as_array().expect("rows array");
    assert_eq!(rows[0]["n"], json!(0), "Bob must see zero rows: {structured}");

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
        .audit_chain_list(tenant.id, Some("bob".to_string()), 1, None)
        .await
        .expect("audit chain list ok");
    let record = records.first().expect("an audit record must exist for bob's filtered read");
    assert_eq!(record.applied, "1 = 0", "the applied predicate must be exactly '1 = 0'");
}
