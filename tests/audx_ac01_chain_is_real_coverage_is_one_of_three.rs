//! PRD-mcphost-audit-export-tamper-evident
//! AC1 (P0) — Given the shipped `host.audit.chain` rows for a fixture tenant
//! and the algorithm as documented in `rowpolicy/audit.rs`, When a short
//! script recomputes `record_hash` for every row, Then it matches, but
//! `tenant_audit` and `oauth_audit` rows carry no hash at all (falsifier:
//! the chain is real, coverage is 1/3).

use crate::common::{McpClient, TestServer, extract_structured, signup};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

/// The documented algorithm, written out independently of
/// `compute_record_hash`: sha256 over the compact, key-sorted JSON of every
/// field except the record's own hash.
fn recompute(r: &mcphost::db::AuditRecordRow) -> String {
    let canonical = json!({
        "subject": r.subject, "tenant_id": r.tenant_id, "plane": r.plane,
        "policy_hash": r.policy_hash, "applied": r.applied,
        "returned_count": r.returned_count, "withheld_count": r.withheld_count,
        "timestamp": r.timestamp_unix, "request_id": r.request_id,
        "prior_record_hash": r.prior_record_hash,
    });
    Sha256::digest(serde_json::to_vec(&canonical).expect("json")).iter().map(|b| format!("{b:02x}")).collect()
}

#[tokio::test]
async fn audx_ac01_chain_recomputes_but_the_other_two_tables_carry_no_hash() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "Audx AC1").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    let tenant = server.state.db.find_tenant_by_namespace(ns).await.expect("find").expect("tenant");
    let now = mcphost::state::now_unix();
    for i in 0..3 {
        server
            .state
            .db
            .audit_chain_append(tenant.id, "alice".into(), "sql".into(), "ph".into(), format!("region = 'r{i}'"), 5, Some(0), now - 10 + i, format!("req{i}"))
            .await
            .expect("append");
    }
    server.state.db.record_tenant_audit(tenant.id, "bob".into(), "key_rotate".into(), None).await.expect("tenant_audit");
    server.state.db.insert_oauth_audit(Some(tenant.id), "token".into(), None, None, None, None, None, None).await.expect("oauth_audit");

    // The stored chain recomputes row by row and links to the genesis.
    let rows = server.state.db.audit_chain_range(tenant.id, 0, i64::MAX).await.expect("range");
    assert_eq!(rows.len(), 3);
    let mut prior = mcphost::rowpolicy::audit::GENESIS_HASH.to_string();
    for r in &rows {
        assert_eq!(r.prior_record_hash, prior, "row {} links to the previous hash", r.id);
        assert_eq!(recompute(r), r.record_hash, "row {} recomputes", r.id);
        prior = r.record_hash.clone();
    }

    // The shipped page itself names no hash: coverage of what a third party can check starts here.
    let chain = extract_structured(&client.tools_call("host.audit.chain", json!({})).await.expect("chain"));
    let first: &Value = &chain["records"][0];
    assert!(first.get("record_hash").is_none() && first.get("prior_record_hash").is_none(), "{first}");

    // Falsifier outcome: only one of the three audit tables carries a hash column.
    let mut hashed = Vec::new();
    for table in ["audit_chain", "tenant_audit", "oauth_audit"] {
        let cols = server.state.db.audit_table_columns_for_test(table).await.expect("columns");
        if cols.iter().any(|c| c == "record_hash" || c == "prior_record_hash") {
            hashed.push(table);
        }
    }
    assert_eq!(hashed, vec!["audit_chain"], "chain is real, coverage is 1/3");
}
