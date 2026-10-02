//! PRD-mcphost-chain-run-lineage AC9 (P0) — Given a tenant with one parent
//! and 3 children, When `host.self_offboard` (or admin tenant delete) runs,
//! Then all 4 run rows are gone; and When `host.runs.purge(parent)` runs,
//! Then the children's results are purged too.
//!
//! "`host.runs.purge(parent)`" reads as the existing `before_unix`-windowed
//! `host.runs.purge` call, given a threshold that covers the parent's (and
//! so its children's) finish time -- `Db::purge_runs` is tenant-scoped, not
//! parent-scoped, so it already clears every `done` row (parent AND
//! children alike) finished at or before that threshold, with no
//! parent-specific code of its own (see `Db::insert_composed_run`'s own
//! doc comment: a composed child is an ordinary `runs` row everywhere else
//! in this crate).

use crate::common;
use common::{chain_kind_registry, publish, signup, ADMIN_KEY, TestServer};
use serde_json::json;

async fn publish_and_run_chain(client: &common::McpClient, ns: &str) {
    let schema = json!({"type": "object"});
    publish(client, "fetch_data", "echo", json!({"schema": schema})).await;
    publish(client, "transform", "echo", json!({"schema": schema})).await;
    publish(client, "write", "echo", json!({"schema": schema})).await;

    let chain_spec = json!({
        "steps": [
            {"tool": "fetch_data", "args": {"url": "$.input.url"}},
            {"tool": "transform", "args": {"rows": "$.prev.result.url"}},
            {"tool": "write", "args": {"rows": "$.prev.result.rows", "region": "$.input.region"}},
        ]
    });
    let chain = publish(client, "daily_pipeline", "chain", chain_spec).await;
    assert_eq!(chain, format!("{ns}.daily_pipeline"));

    client
        .tools_call(&chain, json!({"url": "https://example.com/data", "region": "eu"}))
        .await
        .expect("call ok");
}

#[tokio::test]
async fn tenant_delete_removes_the_parent_and_all_three_children() {
    let server = TestServer::start_with_kinds(chain_kind_registry()).await;
    let (ns, key) = signup(&server.base_url, "AC9 Delete Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);
    publish_and_run_chain(&client, &ns).await;

    let tenant = server
        .state
        .db
        .find_tenant_by_namespace(ns.clone())
        .await
        .expect("find tenant")
        .expect("tenant exists");

    let all_rows = server
        .state
        .db
        .list_runs(tenant.id, None, None, None, None, None, true, 200)
        .await
        .expect("list_runs include_children");
    assert_eq!(all_rows.len(), 4, "one parent + 3 children before delete: {all_rows:?}");

    let admin_client = common::McpClient::with_bearer(&server.base_url, ADMIN_KEY);
    admin_client
        .tools_call("admin.tenant_delete", json!({"tenant": ns}))
        .await
        .expect("tenant_delete ok");

    for row in &all_rows {
        let after = server
            .state
            .db
            .get_run(row.id.clone(), tenant.id)
            .await
            .expect("get_run after delete");
        assert!(after.is_none(), "run '{}' must be gone after tenant_delete", row.id);
    }
}

#[tokio::test]
async fn purging_the_parents_window_clears_every_childs_stored_result_too() {
    let server = TestServer::start_with_kinds(chain_kind_registry()).await;
    let (ns, key) = signup(&server.base_url, "AC9 Purge Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);
    publish_and_run_chain(&client, &ns).await;

    let tenant = server
        .state
        .db
        .find_tenant_by_namespace(ns)
        .await
        .expect("find tenant")
        .expect("tenant exists");

    let runs = server
        .state
        .db
        .list_runs(tenant.id, None, None, None, None, None, false, 200)
        .await
        .expect("list_runs");
    let parent_id = runs[0].id.clone();
    let children_before = server
        .state
        .db
        .get_run_children(tenant.id, parent_id)
        .await
        .expect("get_run_children");
    assert_eq!(children_before.len(), 3);
    for child in &children_before {
        assert!(child.result_ref.is_some(), "a done child must have a stored result before purge");
    }

    let before_unix = mcphost::state::now_unix() + 5;
    client
        .tools_call("host.runs.purge", json!({"before_unix": before_unix}))
        .await
        .expect("purge ok");

    for child in &children_before {
        let after = server
            .state
            .db
            .get_run(child.id.clone(), tenant.id)
            .await
            .expect("get_run after purge")
            .expect("child row still exists after purge");
        assert!(after.result_ref.is_none(), "child '{}' result_ref must be cleared by purge", after.id);
        assert!(after.purged_unix.is_some(), "child '{}' must be marked purged", after.id);
    }
}
