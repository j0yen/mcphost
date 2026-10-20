//! PRD-mcphost-audit-export-tamper-evident
//! AC5 (P0) — Given a stored export root, When `host.audit.verify
//! {root_hash}` runs, Then `{verified: true, rows: 9}`; Given an unknown
//! root, Then `not_found`; Given a tenant with zero rows, When it exports,
//! Then `rows: 0` and the trailer still verifies.

use crate::audx_ac02_export_nine_rows_three_tables::seed_nine;
use crate::audx_ac03_offline_verifier_ok_and_fail_lines::{run_verifier, scratch_dir};
use crate::common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn audx_ac05_verify_answers_by_root_and_an_empty_tenant_exports_a_verifying_trailer() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "Audx AC5").await;
    let (client, tenant_id) = seed_nine(&server, ns, &key).await;

    let out = extract_structured(&client.tools_call("host.audit.export", json!({})).await.expect("export"));
    let root = out["root_hash"].as_str().expect("root").to_string();
    let verified = extract_structured(&client.tools_call("host.audit.verify", json!({"root_hash": root})).await.expect("verify"));
    assert_eq!(verified["verified"], json!(true), "{verified}");
    assert_eq!(verified["rows"], json!(9), "{verified}");
    assert!(verified["through_unix"].is_i64(), "{verified}");

    // A row stamped after the export's second is not part of what it signed.
    tokio::time::sleep(std::time::Duration::from_millis(1_100)).await;
    server.state.db.record_tenant_audit(tenant_id, "bob".into(), "late".into(), None).await.expect("late row");
    let again = extract_structured(&client.tools_call("host.audit.verify", json!({"root_hash": root})).await.expect("verify"));
    assert_eq!(again["verified"], json!(true), "{again}");

    // A row altered afterwards is caught.
    server.state.db.test_tamper_audit_applied(1, "region = 'x'".to_string()).await.expect("tamper");
    let broken = extract_structured(&client.tools_call("host.audit.verify", json!({"root_hash": root})).await.expect("verify"));
    assert_eq!(broken["verified"], json!(false), "{broken}");

    // An unknown root is not_found.
    let err = client
        .tools_call("host.audit.verify", json!({"root_hash": "f".repeat(64)}))
        .await
        .expect_err("unknown root");
    assert_eq!(err.error_code.as_deref(), Some("not_found"), "{err:?}");

    // The chain verify keeps working with and without bounds.
    let chain = extract_structured(&client.tools_call("host.audit.verify", json!({"from_id": 1, "to_id": 3})).await.expect("chain verify"));
    assert_eq!(chain["intact"], json!(false), "the tampered record is named: {chain}");
}

#[tokio::test]
async fn audx_ac05_zero_rows_export_still_verifies() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Audx AC5 empty").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    let jwks: serde_json::Value = reqwest::get(format!("{}/.well-known/jwks.json", server.base_url))
        .await
        .expect("jwks")
        .json()
        .await
        .expect("jwks json");

    let out = extract_structured(&client.tools_call("host.audit.export", json!({})).await.expect("export"));
    assert_eq!(out["rows"], json!(0), "{out}");
    assert_eq!(out["root_hash"], json!(mcphost::rowpolicy::audit::GENESIS_HASH));
    let jsonl = out["jsonl"].as_str().expect("jsonl");
    assert_eq!(jsonl.lines().count(), 2, "a header and a trailer");

    let dir = scratch_dir("ac05");
    let (code, stdout) = run_verifier(&dir, jsonl, &jwks);
    assert_eq!((code, stdout.starts_with("OK 0 rows")), (0, true), "{stdout}");
    let _ = std::fs::remove_dir_all(dir);

    let verified = extract_structured(
        &client.tools_call("host.audit.verify", json!({"root_hash": out["root_hash"]})).await.expect("verify"),
    );
    assert_eq!((verified["verified"].clone(), verified["rows"].clone()), (json!(true), json!(0)));
}
