//! PRD-mcphost-shared-call-run-scope
//! AC7 (P1) — Given three pre-existing owner-scoped sync shared-call rows,
//! When the migration runs, Then they belong to the callers and O's
//! `host.runs.list` no longer shows them.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn migration_0049_backfills_legacy_owner_scoped_shared_runs_to_their_callers() {
    let server = TestServer::start().await;

    let (ns_o, key_o) = signup(&server.base_url, "Owner").await;
    let client_o = McpClient::with_bearer(&server.base_url, &key_o);
    let owner = server
        .state
        .db
        .find_tenant_by_namespace(ns_o.clone())
        .await
        .expect("find O")
        .expect("O exists");

    let mut callers = Vec::new();
    for i in 0..3 {
        let (ns_b, key_b) = signup(&server.base_url, &format!("Caller {i}")).await;
        let tenant_b = server
            .state
            .db
            .find_tenant_by_namespace(ns_b.clone())
            .await
            .expect("find caller")
            .expect("caller exists");
        callers.push((ns_b, key_b, tenant_b.id));
    }

    // Simulate three rows this PRD's fix would never write again: the
    // pre-fix synchronous cross-tenant shape (owner-scoped, caller_tenant_id
    // set, bare local tool name).
    let mut run_ids = Vec::new();
    for (_, _, caller_id) in &callers {
        let run_id = server
            .state
            .db
            .insert_legacy_owner_scoped_shared_run_for_test(owner.id, *caller_id, "lookup".to_string())
            .await
            .expect("insert legacy run");
        run_ids.push(run_id);
    }

    // Precondition: before the migration re-runs, the legacy rows are
    // exactly where the pre-fix bug put them -- under O.
    let owner_runs_before = extract_structured(
        &client_o
            .tools_call("host.runs.list", json!({"limit": 50}))
            .await
            .expect("O lists its own runs before migration"),
    );
    assert_eq!(
        owner_runs_before["runs"].as_array().expect("runs array").len(),
        3,
        "precondition: three legacy rows must be owner-scoped before the migration runs: {owner_runs_before}"
    );

    server.state.db.migrate().await.expect("re-run migrations (0049 backfill)");

    let owner_runs_after = extract_structured(
        &client_o
            .tools_call("host.runs.list", json!({"limit": 50}))
            .await
            .expect("O lists its own runs after migration"),
    );
    assert!(
        owner_runs_after["runs"].as_array().expect("runs array").is_empty(),
        "O's host.runs.list must no longer show the backfilled legacy rows: {owner_runs_after}"
    );

    let qualified = format!("{ns_o}.lookup");
    for (i, (_, key_b, _)) in callers.iter().enumerate() {
        let client_b = McpClient::with_bearer(&server.base_url, key_b);
        let caller_runs = extract_structured(
            &client_b
                .tools_call("host.runs.list", json!({}))
                .await
                .unwrap_or_else(|_| panic!("caller {i} lists its own runs")),
        );
        let list = caller_runs["runs"].as_array().expect("runs array");
        assert_eq!(list.len(), 1, "caller {i} must now own exactly its backfilled run: {caller_runs}");
        assert_eq!(list[0]["tool"], json!(qualified), "caller {i}'s run must carry the qualified tool name");
        assert_eq!(list[0]["trigger"], json!("call"));
        assert_eq!(list[0]["run_id"], json!(run_ids[i]), "caller {i} must own the same run id, just rescoped");
    }
}
