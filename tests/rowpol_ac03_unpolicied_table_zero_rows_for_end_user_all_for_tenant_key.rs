//! PRD-mcphost-row-policy
//! AC3 (P0) — Given a table with no policy, When an end user queries it,
//! Then zero rows return; When the tenant key queries it, Then all rows
//! return and no audit record is written.

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
async fn unpolicied_table_is_zero_rows_for_an_end_user_and_untouched_for_the_tenant_key() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "AC3 Tenant").await;
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
    // Deliberately no host.policy.set call at all for this table.

    let tenant = server
        .state
        .db
        .find_tenant_by_namespace(ns)
        .await
        .expect("db ok")
        .expect("tenant exists");

    let assertion = sign_assertion(&secret, "carol");
    let end_user_result = client
        .tools_call(
            "host.table.query",
            json!({"sql": "SELECT COUNT(*) AS n FROM orders", "end_user_assertion": assertion}),
        )
        .await
        .unwrap_or_else(|e| panic!("carol's query must still succeed: {} {}", e.code, e.message));
    let structured = extract_structured(&end_user_result);
    assert_eq!(
        structured["rows"][0]["n"],
        json!(0),
        "an end user on an unpolicied table must see zero rows: {structured}"
    );

    let (_records, audit_total_after_end_user) = server
        .state
        .db
        .audit_chain_list(tenant.id, None, 1000, None)
        .await
        .expect("audit chain list ok");

    let tenant_key_result = client
        .tools_call("host.table.query", json!({"sql": "SELECT COUNT(*) AS n FROM orders"}))
        .await
        .expect("tenant key query must succeed");
    let tenant_key_structured = extract_structured(&tenant_key_result);
    assert_eq!(
        tenant_key_structured["rows"][0]["n"],
        json!(2),
        "the tenant key must see every row on an unpolicied table: {tenant_key_structured}"
    );

    let (_records, audit_total_after_tenant_key) = server
        .state
        .db
        .audit_chain_list(tenant.id, None, 1000, None)
        .await
        .expect("audit chain list ok");
    assert_eq!(
        audit_total_after_end_user, audit_total_after_tenant_key,
        "a tenant-key call must write no audit record"
    );
}
