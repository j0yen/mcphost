//! PRD-mcphost-delegation-bound-to-run-or-session
//! AC3 (P0) — Given the watcher key, When run `R` reaches `done`, Then within
//! the same transaction `tenant_keys.revoked_at` is set, an audit row
//! `revoke_bound`/`run_done` exists, and the watcher's next call fails
//! `key_revoked` with `data.reason == "run_done"`.

use crate::common::{McpClient, TestServer, extract_structured, signup};
use serde_json::{Value, json};

#[tokio::test]
async fn kbind_ac03_run_done_revokes_the_watcher_key_in_the_finalize_transaction() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Kbind Revoke Tenant").await;
    let primary = McpClient::with_bearer(&server.base_url, &key);
    let db = &server.state.db;
    let tenant = db
        .find_tenant_by_key_hash(mcphost::auth::hash_key(&key))
        .await
        .expect("find tenant")
        .expect("tenant");
    let run_id = mcphost::state::new_ulid();
    db.insert_queued_run(
        run_id.clone(), tenant.id, "watched".into(), "job".into(), None, None, 300, "{}".into(),
        false, false, None, None, None, None,
    )
    .await
    .expect("insert run");

    let delegated = extract_structured(
        &primary
            .tools_call("host.key.delegate", json!({"bind": {"run_id": run_id}, "label": "watcher"}))
            .await
            .expect("delegate"),
    );
    let child = McpClient::with_bearer(&server.base_url, delegated["key"].as_str().expect("key"));
    child.tools_call("host.runs.get", json!({"run_id": run_id})).await.expect("works while the run is open");

    // The executor's own terminal write: one call, one transaction.
    let finished_unix = mcphost::state::now_unix();
    let finalized = db
        .finalize_run(run_id.clone(), tenant.id, "done".into(), None, None, None, finished_unix, 5)
        .await
        .expect("finalize");
    assert!(finalized);

    let listed = extract_structured(&primary.tools_call("host.key.list", json!({})).await.expect("list"));
    let watcher = &listed["keys"][0];
    assert_eq!(watcher["label"], json!("watcher"), "{listed}");
    assert_eq!(watcher["revoked_at"], json!(finished_unix), "{listed}");

    let audit = db
        .list_tenant_audit_for_subject(tenant.id, "key:watcher".into(), None, 10)
        .await
        .expect("audit");
    let row = audit.iter().find(|(_, action, _)| action == "revoke_bound").unwrap_or_else(|| panic!("{audit:?}"));
    let detail: Value = serde_json::from_str(row.2.as_deref().expect("detail")).expect("json detail");
    assert_eq!(detail["reason"], json!("run_done"), "{detail}");

    let err = child
        .tools_call("host.runs.get", json!({"run_id": run_id}))
        .await
        .expect_err("revoked after the run ended");
    assert_eq!(err.error_code.as_deref(), Some("key_revoked"), "{err:?}");
    assert_eq!(err.data["reason"], json!("run_done"), "{err:?}");
}
