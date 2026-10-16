//! PRD-mcphost-delegation-bound-to-run-or-session
//! AC6 (P1) — Given the watcher key, When `host.key.list` and `host.whoami`
//! run, Then both show `bound == {kind: "run", ref: R, last_seen_unix}` and,
//! after the run ends, `revoked_reason == "run_done"`.

use crate::common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn kbind_ac06_key_list_and_whoami_carry_bound_and_revoked_reason() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Kbind Show Tenant").await;
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

    let check_bound = |v: &serde_json::Value| {
        assert_eq!(v["bound"]["kind"], json!("run"), "{v}");
        assert_eq!(v["bound"]["ref"], json!(run_id), "{v}");
        assert!(v["bound"]["last_seen_unix"].is_i64(), "{v}");
    };

    let listed = extract_structured(&primary.tools_call("host.key.list", json!({})).await.expect("list"));
    check_bound(&listed["keys"][0]);
    assert_eq!(listed["keys"][0]["revoked_reason"], json!(null), "{listed}");

    let me = extract_structured(&child.tools_call("host.whoami", json!({})).await.expect("whoami"));
    check_bound(&me["key"]);

    db.finalize_run(run_id.clone(), tenant.id, "done".into(), None, None, None, mcphost::state::now_unix(), 1)
        .await
        .expect("finalize");
    let listed = extract_structured(&primary.tools_call("host.key.list", json!({})).await.expect("list"));
    check_bound(&listed["keys"][0]);
    assert_eq!(listed["keys"][0]["revoked_reason"], json!("run_done"), "{listed}");
}
