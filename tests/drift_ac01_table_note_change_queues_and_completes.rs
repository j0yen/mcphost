//! PRD-mcphost-drift-review
//! AC1 -- Given a table note change through `host.table.model_set`, When
//! the tick runs, Then `context_versions` has a new row for that target
//! and a `drift_queue` entry with status `queued`, then `done` within 60s.
//!
//! Drives `tables_model::tick_once` directly (which rides `drift::rerun`'s
//! own queue drain on the same cycle) rather than waiting on the real 10s
//! background cadence -- same deterministic-tick convention
//! `tablemodel_ac05_stale_after_append_recomputes_on_tick.rs` already uses.

use crate::common;
use common::{McpClient, TestServer, signup};
use serde_json::json;

#[tokio::test]
async fn table_note_change_queues_then_completes_within_the_tick() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Drift AC1 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    let tenant = server
        .state
        .db
        .find_tenant_by_namespace(_ns.clone())
        .await
        .expect("find tenant")
        .expect("tenant exists");

    client
        .tools_call(
            "host.table.create",
            json!({"name": "expenses", "columns": {"amount": "real"}}),
        )
        .await
        .expect("create expenses");

    // No version recorded before the first note change.
    let before = server
        .state
        .db
        .latest_context_version(tenant.id, "table_note".to_string(), "expenses".to_string())
        .await
        .expect("latest_context_version");
    assert_eq!(before, None, "no table_note version should exist yet: {before:?}");

    client
        .tools_call(
            "host.table.model_set",
            json!({"table": "expenses", "key": "description", "value": "USD amounts"}),
        )
        .await
        .expect("model_set");

    // `context_versions` has a new row for this target.
    let after = server
        .state
        .db
        .latest_context_version(tenant.id, "table_note".to_string(), "expenses".to_string())
        .await
        .expect("latest_context_version");
    assert_eq!(after, Some(1), "a table_note version row must exist after model_set: {after:?}");

    // A `drift_queue` entry exists with status `queued`.
    let queued = server.state.db.list_queued_drift().await.expect("list_queued_drift");
    assert!(
        queued.iter().any(|q| q.tenant_id == tenant.id && q.target == "expenses" && q.kind == "table_note"),
        "expected a queued drift entry for expenses/table_note: {queued:?}"
    );

    // The tick processes it to `done` within this one cycle.
    mcphost::tables_model::tick_once(&server.state).await.expect("tick_once");

    let queued_after = server.state.db.list_queued_drift().await.expect("list_queued_drift");
    assert!(
        !queued_after.iter().any(|q| q.tenant_id == tenant.id && q.target == "expenses"),
        "the drift queue entry must have reached done after the tick: {queued_after:?}"
    );
}
