//! PRD-mcphost-acting-for-in-ledger-and-audit
//! AC5 (P0) — Given the same action performed by the root and by a child,
//! When both `audit_chain` records are read, Then their `record_hash` differ
//! and the child's carries `acting_for.label`.

use crate::common::{McpClient, TestServer, extract_structured, signup};
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use mcphost::rowpolicy::audit::{GENESIS_HASH, compute_record_hash, compute_record_hash_ext};
use serde::Serialize;
use serde_json::{Value, json};

#[derive(Serialize)]
struct AssertionClaims {
    sub: String,
    iat: i64,
    exp: i64,
}

fn sign_assertion(secret: &str, sub: &str) -> String {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64;
    let claims = AssertionClaims { sub: sub.to_string(), iat: now, exp: now + 300 };
    encode(&Header::new(Algorithm::HS256), &claims, &EncodingKey::from_secret(secret.as_bytes()))
        .expect("sign test assertion")
}

#[test]
fn actfor_ac05_hash_commits_to_acting_for_and_old_records_keep_their_hash() {
    let args = ("alice", 1, "sql", "ph", "region = 'EU'", 3, Some(0), 1000, "req", GENESIS_HASH);
    let plain = compute_record_hash(args.0, args.1, args.2, args.3, args.4, args.5, args.6, args.7, args.8, args.9);
    let ext = |af: Option<&str>| {
        compute_record_hash_ext(
            args.0, args.1, args.2, args.3, args.4, args.5, args.6, args.7, args.8, args.9, None, None, af,
        )
    };
    assert_eq!(ext(None), plain, "an absent field leaves a pre-PRD record's hash alone");
    let child = ext(Some(r#"{"key_id":7,"label":"deployer","parent_label":null,"root_label":"deployer","depth":1}"#));
    assert_ne!(child, plain, "identical content by a child must hash differently");
}

#[tokio::test]
async fn actfor_ac05_same_read_by_root_and_child_yields_distinct_hashes() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "ActFor Chain Tenant").await;
    let primary = McpClient::with_bearer(&server.base_url, &key);
    let rotate = primary.tools_call("host.enduser.assertion_secret_rotate", json!({})).await.expect("rotate");
    let secret = extract_structured(&rotate)["secret"].as_str().expect("secret").to_string();
    primary
        .tools_call(
            "host.table.create",
            json!({"name": "orders", "columns": {"id": "integer", "region": "text"}, "primary_key": "id"}),
        )
        .await
        .expect("table create");
    primary
        .tools_call("host.table.append", json!({"table": "orders", "rows": [{"id": 1, "region": "EU"}]}))
        .await
        .expect("append");
    primary
        .tools_call(
            "host.policy.set",
            json!({"target": {"table": "orders"}, "rule": [{"column_or_attr": "region", "op": "eq", "value": {"attr": "region"}}]}),
        )
        .await
        .expect("policy set");
    primary
        .tools_call("host.policy.attrs_set", json!({"subject": "alice", "attrs": {"region": "EU"}}))
        .await
        .expect("attrs set");

    let out: Value = extract_structured(
        &primary
            .tools_call(
                "host.key.delegate",
                json!({"scopes": ["tools:call", "tools:write"], "ttl_s": 600, "label": "deployer"}),
            )
            .await
            .expect("delegate"),
    );
    let child = McpClient::with_bearer(&server.base_url, out["key"].as_str().expect("key"));

    let query = json!({"sql": "SELECT COUNT(*) AS n FROM orders", "end_user_assertion": sign_assertion(&secret, "alice")});
    primary.tools_call("host.table.query", query.clone()).await.expect("root read");
    child.tools_call("host.table.query", query).await.expect("child read");

    let chain = extract_structured(
        &primary.tools_call("host.audit.chain", json!({"subject": "alice"})).await.expect("chain"),
    );
    let records = chain["records"].as_array().expect("records");
    assert_eq!(records.len(), 2, "{chain}");
    // Newest first: the child's read, then the root's.
    assert_eq!(records[0]["acting_for"]["label"], json!("deployer"), "{chain}");
    assert!(records[1].get("acting_for").is_none(), "{chain}");

    let conn = rusqlite::Connection::open(server.data_dir.0.join("mcphost.db")).expect("raw db");
    let hashes: Vec<String> = conn
        .prepare("SELECT record_hash FROM audit_chain WHERE subject = 'alice' ORDER BY id")
        .expect("prepare")
        .query_map([], |r| r.get(0))
        .expect("query")
        .collect::<Result<_, _>>()
        .expect("rows");
    assert_eq!(hashes.len(), 2);
    assert_ne!(hashes[0], hashes[1]);

    let ids: Vec<i64> = records.iter().map(|r| r["id"].as_i64().expect("id")).collect();
    let verify = extract_structured(
        &primary
            .tools_call("host.audit.verify", json!({"from_id": ids.iter().min().unwrap(), "to_id": ids.iter().max().unwrap()}))
            .await
            .expect("verify"),
    );
    assert_eq!(verify["ok"], json!(true), "{verify}");
}
