//! PRD-mcphost-delegation-bound-to-run-or-session
//! AC7 (P0) — Given a run with `deadline` in 300 s, When the primary delegates
//! `{bind: {run_id}, ttl_s: 900}`, Then the child's `expires_at` equals the
//! run's deadline, not now+900.

use crate::common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn kbind_ac07_child_expires_at_the_runs_deadline_not_at_the_requested_ttl() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Kbind Deadline Tenant").await;
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
    // Leasing stamps `started_unix`, which fixes the deadline: started + 300.
    let leased = db.lease_next_queued_run(tenant.id).await.expect("lease").expect("leased run");
    let deadline = leased.started_unix.expect("started_unix") + 300;

    let delegated = extract_structured(
        &primary
            .tools_call(
                "host.key.delegate",
                json!({"scopes": ["mcp"], "ttl_s": 900, "label": "watcher", "bind": {"run_id": run_id}}),
            )
            .await
            .expect("delegate"),
    );
    assert_eq!(delegated["expires_at"], json!(deadline), "{delegated}");
    let listed = extract_structured(&primary.tools_call("host.key.list", json!({})).await.expect("list"));
    assert_eq!(listed["keys"][0]["expires_at"], json!(deadline), "{listed}");

    // Without a bind the same request still gets the full 900 s.
    let plain = extract_structured(
        &primary
            .tools_call("host.key.delegate", json!({"scopes": ["mcp"], "ttl_s": 900, "label": "plain"}))
            .await
            .expect("plain delegate"),
    );
    assert!(plain["expires_at"].as_i64().expect("expires_at") > deadline + 400, "{plain}");
}
