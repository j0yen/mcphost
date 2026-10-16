//! PRD-mcphost-delegation-bound-to-run-or-session
//! AC4 (P0) — Given `bind: {run_id: "nope"}` or a run owned by another
//! tenant, When delegation runs, Then `not_found` and no row; Given `bind:
//! {session_id: "<other connection's id>"}`, Then `permission_denied`.

use crate::common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

async fn tenant_id(server: &TestServer, key: &str) -> i64 {
    server
        .state
        .db
        .find_tenant_by_key_hash(mcphost::auth::hash_key(key))
        .await
        .expect("find tenant")
        .expect("tenant")
        .id
}

#[tokio::test]
async fn kbind_ac04_unknown_foreign_run_not_found_foreign_session_permission_denied() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Kbind Target Tenant").await;
    let (_ns2, other_key) = signup(&server.base_url, "Kbind Other Tenant").await;
    let mine = tenant_id(&server, &key).await;
    let theirs = tenant_id(&server, &other_key).await;
    let a = McpClient::with_bearer(&server.base_url, &key).with_session_continuity();
    let b = McpClient::with_bearer(&server.base_url, &key).with_session_continuity();

    let foreign_run = mcphost::state::new_ulid();
    server
        .state
        .db
        .insert_queued_run(
            foreign_run.clone(), theirs, "watched".into(), "job".into(), None, None, 300, "{}".into(),
            false, false, None, None, None, None,
        )
        .await
        .expect("insert other tenant's run");

    for run_id in ["nope", foreign_run.as_str()] {
        let err = a
            .tools_call("host.key.delegate", json!({"bind": {"run_id": run_id}, "label": "watcher"}))
            .await
            .expect_err("unknown / foreign run is refused");
        assert_eq!(err.error_code.as_deref(), Some("not_found"), "{run_id}: {err:?}");
    }

    // B's session id, as A learned it (A's `host.whoami` on B's behalf would
    // be the same id B reads), is not A's to bind to.
    let who_b = b.tools_call("host.whoami", json!({})).await.expect("whoami b");
    let sid_b = who_b["_meta"]["session_id"].as_str().expect("B session id").to_string();
    let err = a
        .tools_call("host.key.delegate", json!({"bind": {"session_id": sid_b}, "label": "stolen"}))
        .await
        .expect_err("another connection's session id is refused");
    assert_eq!(err.error_code.as_deref(), Some("permission_denied"), "{err:?}");

    assert_eq!(server.state.db.count_sub_keys(mine).await.expect("count"), 0, "no key row was written");
    let listed = extract_structured(&a.tools_call("host.key.list", json!({})).await.expect("list"));
    assert_eq!(listed["keys"], json!([]), "{listed}");
}
