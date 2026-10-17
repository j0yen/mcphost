//! PRD-mcphost-graceful-drain-contract-b AC2 -- Given an async run `running`
//! at signal, When the drain ends without it, Then its `runs` row reads
//! `status = 'error', error_class = 'interrupted'` before exit, and a
//! scheduler tick after restart does not insert a skipped run for its trigger.
//!
//! Drives `Db::interrupt_running_runs` (the call `serve_on_listener`'s drain
//! makes ahead of the `restarts` write) on a bare `AppState`, then one
//! scheduler tick, the same shape as `sched_ac5`'s overlap fixture.

use crate::common;
use serde_json::json;

#[tokio::test]
async fn gdrainb_ac02() {
    let (state, _data_dir) = common::bare_app_state().await;
    let tenant = state
        .db
        .create_tenant("GDB AC2".to_string(), "t-gdb-ac2".to_string(), "key-hash-gdb-ac2".to_string(), None)
        .await
        .expect("create_tenant");
    state
        .db
        .upgrade_tenant_plan(tenant.id, "pro".to_string(), mcphost::state::rfc3339_now(), None)
        .await
        .expect("upgrade to pro");
    let tenant = state.db.find_tenant_by_id(tenant.id).await.expect("reload").expect("tenant exists");
    mcphost::control::tool_publish(
        &state,
        &tenant,
        &json!({"name": "slow", "kind": "echo", "spec": {"schema": {"type": "object"}}}),
    )
    .await
    .expect("publish");
    let set = mcphost::triggers::set(&state, &tenant, &json!({"tool": "slow", "schedule": "* * * * *"}))
        .await
        .expect("trigger.set");
    let trigger_id = set["id"].as_str().expect("id").to_string();

    let run_id = mcphost::state::new_ulid();
    state
        .db
        .insert_queued_run(
            run_id.clone(),
            tenant.id,
            "slow".to_string(),
            "schedule".to_string(),
            Some(trigger_id.clone()),
            None,
            900,
            "{}".to_string(),
            false,
            false,
            None,
            None,
            None,
            None,
        )
        .await
        .expect("insert run");
    let leased = state.db.lease_next_queued_run(tenant.id).await.expect("lease").expect("a queued run");
    assert_eq!(leased.id, run_id);
    assert_eq!(state.db.get_run(run_id.clone(), tenant.id).await.unwrap().unwrap().status, "running");

    // The drain ends without it.
    assert_eq!(state.db.interrupt_running_runs().await.expect("interrupt"), 1);
    let row = state.db.get_run(run_id.clone(), tenant.id).await.unwrap().unwrap();
    assert_eq!(row.status, "error");
    assert_eq!(row.error_class.as_deref(), Some("interrupted"));
    assert_eq!(state.db.interrupt_running_runs().await.expect("second pass is a no-op"), 0);

    // After restart: the trigger comes due, a tick must not skip it.
    let now = mcphost::state::now_unix();
    state
        .db
        .update_trigger_after_fire(trigger_id.clone(), Some(now), None, 0)
        .await
        .expect("force due");
    mcphost::triggers::tick_once(&state).await.expect("tick");
    let skipped = state
        .db
        .list_runs(tenant.id, None, Some("skipped".to_string()), Some("schedule".to_string()), None, None, false, 20)
        .await
        .expect("list_runs");
    assert!(
        skipped.iter().all(|r| r.trigger_ref.as_deref() != Some(trigger_id.as_str())),
        "no skipped run for trigger {trigger_id}: {skipped:?}"
    );
}
