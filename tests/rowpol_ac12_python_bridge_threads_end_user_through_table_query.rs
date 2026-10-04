//! PRD-mcphost-row-policy
//! AC12 (P1) -- Given a `python` tool run as Alice, When it calls
//! `mcphost.table.query` on `orders`, Then it receives EU rows only.
//!
//! `mcphost.table.query` reaches `tables::table_query` through
//! `TenantTableBridge` (see `tests/tables_ac05_python_sandbox_table_access.rs`
//! for the same sandbox-channel shape without row policy), which previously
//! hardcoded `end_user: None` -- a row policy never applied inside a python
//! sandbox no matter which end user the top-level call carried. This proves
//! the fix: an `end_user_assertion`-carrying sandboxed call to
//! `mcphost.table.query` is scoped exactly like the same call made directly
//! to `host.table.query` (AC1).

use crate::common;
use common::{McpClient, TestServer, extract_structured, poll_until_ready, python_kind_registry, signup};
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use mcphost::sandbox;
use serde::Serialize;
use serde_json::json;
use std::time::Duration;

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
async fn mcphost_table_query_from_a_sandboxed_tool_is_scoped_to_the_calling_end_user() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs_dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (ns, key) = signup(&server.base_url, "AC12 Tenant").await;
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
            json!({
                "table": "orders",
                "rows": [
                    {"id": 1, "region": "EU"},
                    {"id": 2, "region": "US"},
                    {"id": 3, "region": "EU"},
                ],
            }),
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

    let source = r#"import mcphost
def main(args):
    result = mcphost.table.query(sql="SELECT id, region FROM orders ORDER BY id")
    return {"rows": result["rows"]}
"#;
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "orders_reader", "kind": "python", "spec": {"source": source}}),
        )
        .await
        .expect("publish ok");

    let qualified = format!("{ns}.orders_reader");
    let assertion = sign_assertion(&secret, "alice");
    let called = poll_until_ready(
        &client,
        &qualified,
        json!({"end_user_assertion": assertion}),
        Duration::from_secs(10),
    )
    .await
    .unwrap_or_else(|e| panic!("sandboxed, end-user-scoped table query must succeed: {} {}", e.code, e.message));
    let structured = extract_structured(&called);
    let rows = structured["rows"].as_array().expect("rows array");
    assert_eq!(rows.len(), 2, "only alice's EU rows must return: {rows:?}");
    for row in rows {
        assert_eq!(row["region"], json!("EU"), "{row:?}");
    }
}
