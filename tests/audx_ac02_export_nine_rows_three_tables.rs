//! PRD-mcphost-audit-export-tamper-evident
//! AC2 (P0) — Given a tenant with 3 `audit_chain`, 4 `tenant_audit` and 2
//! `oauth_audit` rows, When `host.audit.export {}` runs, Then 9 lines in
//! `(ts, table, id)` order, each `hash` recomputes, line 1's `prev` is the
//! genesis, and the trailer `root_hash` equals line 9's `hash`.

use crate::common::{McpClient, TestServer, extract_structured, signup};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

/// Seeds 3 + 4 + 2 rows for the tenant of `ns` and returns the client.
pub(crate) async fn seed_nine(server: &TestServer, ns: String, key: &str) -> (McpClient, i64) {
    let client = McpClient::with_bearer(&server.base_url, key);
    let tenant = server.state.db.find_tenant_by_namespace(ns).await.expect("find").expect("tenant");
    let now = mcphost::state::now_unix();
    for i in 0..3 {
        server
            .state
            .db
            .audit_chain_append(tenant.id, "alice".into(), "sql".into(), "ph".into(), format!("region = 'r{i}'"), 5, Some(0), now - 100 + i, format!("req{i}"))
            .await
            .expect("append");
    }
    for action in ["key_rotate", "scope_grant", "offboard", "key_rotate"] {
        server.state.db.record_tenant_audit(tenant.id, "bob".into(), action.into(), Some("détail \"x\"".into())).await.expect("tenant_audit");
    }
    for event in ["token", "consent"] {
        server.state.db.insert_oauth_audit(Some(tenant.id), event.into(), Some("cid".into()), None, None, None, None, None).await.expect("oauth_audit");
    }
    (client, tenant.id)
}

/// The documented algorithm, independent of `canonical_json`: sha256 over
/// `prev` followed by the compact, key-sorted JSON of the line minus `hash`.
pub(crate) fn recompute(line: &Value) -> String {
    let mut body = line.clone();
    body.as_object_mut().expect("object").remove("hash");
    let prev = line["prev"].as_str().expect("prev");
    let canonical = serde_json::to_string(&body).expect("json");
    Sha256::digest(format!("{prev}{canonical}").as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
}

pub(crate) fn parse_lines(jsonl: &str) -> Vec<Value> {
    jsonl.lines().map(|l| serde_json::from_str(l).expect("jsonl line")).collect()
}

#[tokio::test]
async fn audx_ac02_export_merges_three_tables_in_order_and_each_hash_recomputes() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "Audx AC2").await;
    let (client, _tenant_id) = seed_nine(&server, ns, &key).await;

    let out = extract_structured(&client.tools_call("host.audit.export", json!({})).await.expect("export"));
    assert_eq!(out["rows"], json!(9), "{out}");
    let all = parse_lines(out["jsonl"].as_str().expect("jsonl"));
    assert_eq!(all.len(), 11, "header + 9 rows + trailer");
    let (header, rows, trailer) = (&all[0], &all[1..10], &all[10]);
    assert!(header["tenant_ns"].is_string() && header["kid"].is_string() && header["host_version"].is_string(), "{header}");

    // (ts, table, id) order, and all three tables are present.
    let keys: Vec<(i64, String, i64)> =
        rows.iter().map(|r| (r["ts"].as_i64().unwrap(), r["table"].as_str().unwrap().to_string(), r["id"].as_i64().unwrap())).collect();
    let mut sorted = keys.clone();
    sorted.sort();
    assert_eq!(keys, sorted, "lines are in (ts, table, id) order");
    for (table, n) in [("audit_chain", 3), ("tenant_audit", 4), ("oauth_audit", 2)] {
        assert_eq!(rows.iter().filter(|r| r["table"] == json!(table)).count(), n, "{table}");
    }

    // Each hash recomputes; the chain links from the genesis; the trailer names the last hash.
    let mut prev = mcphost::rowpolicy::audit::GENESIS_HASH.to_string();
    for (i, r) in rows.iter().enumerate() {
        assert_eq!(r["prev"], json!(prev), "line {} prev", i + 1);
        assert_eq!(recompute(r), r["hash"].as_str().unwrap(), "line {} recomputes", i + 1);
        prev = r["hash"].as_str().unwrap().to_string();
    }
    assert_eq!(rows[0]["prev"], json!(mcphost::rowpolicy::audit::GENESIS_HASH));
    assert_eq!(trailer["root_hash"], rows[8]["hash"]);
    assert_eq!(trailer["rows"], json!(9));

    // A stored audit_chain row keeps its own hash in detail.stored_hash.
    let chain_row = rows.iter().find(|r| r["table"] == json!("audit_chain")).expect("chain row");
    assert_eq!(chain_row["detail"]["stored_hash"].as_str().map(str::len), Some(64), "{chain_row}");

    // R5: two exports of the same window are byte-identical except the trailer's
    // signature (ES256 is randomised); the root they sign is the same.
    let again = extract_structured(&client.tools_call("host.audit.export", json!({})).await.expect("export again"));
    let again_lines = parse_lines(again["jsonl"].as_str().expect("jsonl"));
    assert_eq!(again_lines[..10], all[..10], "header and rows are byte-identical");
    assert_eq!(again_lines[10]["root_hash"], trailer["root_hash"]);
}
