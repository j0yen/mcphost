//! PRD-mcphost-delegation-bound-to-run-or-session
//! AC2 (P0) — Given an open async run `R` owned by the tenant, When the
//! primary delegates `{bind: {run_id: R}, label: "watcher"}` and the child
//! calls `host.runs.get {run_id: R}`, Then it succeeds; When the child calls
//! `host.tool.list` with no run id, Then `key_bound_elsewhere` with
//! `data.seen.ref == null` and no `calls` row.

use crate::common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

/// Queue an async run for the tenant behind `key`; the bare test server runs
/// no executor, so it stays open. Returns `(tenant_id, run_id)`.
async fn open_run(server: &TestServer, key: &str) -> (i64, String) {
    let tenant = server
        .state
        .db
        .find_tenant_by_key_hash(mcphost::auth::hash_key(key))
        .await
        .expect("find tenant")
        .expect("tenant");
    let run_id = mcphost::state::new_ulid();
    server
        .state
        .db
        .insert_queued_run(
            run_id.clone(), tenant.id, "watched".into(), "job".into(), None, None, 300, "{}".into(),
            false, false, None, None, None, None,
        )
        .await
        .expect("insert run");
    (tenant.id, run_id)
}

#[tokio::test]
async fn kbind_ac02_run_bound_key_works_with_its_run_id_and_is_refused_without() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Kbind Run Tenant").await;
    let primary = McpClient::with_bearer(&server.base_url, &key);
    let (tenant_id, run_id) = open_run(&server, &key).await;

    let delegated = extract_structured(
        &primary
            .tools_call("host.key.delegate", json!({"bind": {"run_id": run_id}, "label": "watcher"}))
            .await
            .expect("delegate bound to run"),
    );
    assert_eq!(delegated["bound"], json!({"kind": "run", "ref": run_id}), "{delegated}");
    let child = McpClient::with_bearer(&server.base_url, delegated["key"].as_str().expect("key"));

    let got = extract_structured(&child.tools_call("host.runs.get", json!({"run_id": run_id})).await.expect("get"));
    assert_eq!(got["run_id"], json!(run_id), "{got}");

    let before = server.state.db.count_calls_since(tenant_id, 0, false).await.expect("count");
    let err = child.tools_call("host.tool.list", json!({})).await.expect_err("no run id is refused");
    assert_eq!(err.error_code.as_deref(), Some("key_bound_elsewhere"), "{err:?}");
    assert_eq!(err.data["bound"]["kind"], json!("run"), "{err:?}");
    assert_eq!(err.data["bound"]["ref"], json!(run_id), "{err:?}");
    assert_eq!(err.data["seen"]["ref"], json!(null), "{err:?}");
    let after = server.state.db.count_calls_since(tenant_id, 0, false).await.expect("count");
    assert_eq!(before, after, "a refused bound call must not write a calls row");

    // Another run's id is still the wrong run.
    let err = child
        .tools_call("host.runs.get", json!({"run_id": "01OTHERRUN"}))
        .await
        .expect_err("another run id is refused");
    assert_eq!(err.error_code.as_deref(), Some("key_bound_elsewhere"), "{err:?}");
    assert_eq!(err.data["seen"]["ref"], json!("01OTHERRUN"), "{err:?}");
}
