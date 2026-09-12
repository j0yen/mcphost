//! AC5 (P0) — Given a schedule whose tool takes 100s and fires every
//! minute, When three minutes pass, Then the ledger shows one `done` run
//! and at least one run with `status='skipped'`, `error_class='overlap'`.
//!
//! Rather than a real 100s-sleeping tool and a real 3-minute wait, this
//! fabricates the "previous firing still running" precondition directly:
//! it inserts a `queued` run already attached to the trigger (`trigger_ref
//! = id`) before forcing the trigger due and ticking once. That is exactly
//! the state `tick_once`'s overlap check (`last_run_status_for_trigger`)
//! looks at -- the property under test (a still-`queued`/`running`
//! previous firing causes the next one to skip, recorded `status =
//! 'skipped'`, `error_class = 'overlap'`) doesn't depend on how that
//! previous run got there, only that it's still non-terminal.
//!
//! Deliberately builds a bare `AppState` directly (no HTTP listener, no
//! `runs::spawn_executor`/`triggers::spawn_scheduler`), the same reasoning
//! `runs_ac11_admin_runs_reap.rs` already documents: a live background
//! executor would race this test's own manual stranding of a `queued` run
//! (it would lease and finalize `slow`'s fabricated run before the
//! overlap check ever reads it back).

use crate::common;
use mcphost::db::Db;
use mcphost::kinds::KindRegistry;
use mcphost::secrets::SecretBox;
use mcphost::state::AppState;
use serde_json::json;
use std::sync::Arc;

async fn bare_state() -> (AppState, common::TempDataDir) {
    let data_dir = common::TempDataDir::new();
    let db = Db::open(&data_dir.0).expect("open db");
    db.migrate().await.expect("migrate");
    let state = AppState {
        db,
        kinds: KindRegistry::with_builtin(),
        secrets: SecretBox::from_passphrase("test-secret-key"),
        admin_key: Some("test-admin-key".to_string()),
        public_url: "http://127.0.0.1:0".to_string(),
        call_timeout: mcphost::state::CALL_TIMEOUT,
        registry: None,
        http_client: reqwest::Client::new(),
        sandbox_mechanism: None,
        tool_run_limiter: mcphost::state::ToolRunLimiter::new(),
        signup_rate_limit_per_hour: mcphost::state::SIGNUP_RATE_LIMIT_PER_HOUR,
        plans: mcphost::plans::PlanCatalog::default_catalog(),
        billing_config: mcphost::billing::BillingConfig::default(),
        billing_client: Arc::new(mcphost::billing::FakeBillingClient::new(mcphost::state::now_unix())),
        checkout_sessions: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
        accepted_usage_cache: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
        runs: mcphost::runs::RunsRegistry::new(),
        scheduler: mcphost::triggers::SchedulerStatus::new(),
        event_counters: mcphost::hooks::EventCounters::new(),
        event_rate_limiter: mcphost::hooks::EventRateLimiter::new(),
    };
    (state, data_dir)
}

#[tokio::test]
async fn overlapping_firing_is_recorded_as_skipped_overlap() {
    let (state, _data_dir) = bare_state().await;

    let tenant = state
        .db
        .create_tenant(
            "AC5 Tenant".to_string(),
            "t-ac5".to_string(),
            "key-hash-ac5".to_string(),
            None,
        )
        .await
        .expect("create_tenant");
    state
        .db
        .upgrade_tenant_plan(tenant.id, "pro".to_string(), mcphost::state::rfc3339_now(), None)
        .await
        .expect("upgrade to pro");
    let tenant = state
        .db
        .find_tenant_by_id(tenant.id)
        .await
        .expect("reload tenant")
        .expect("tenant exists");

    mcphost::control::tool_publish(
        &state,
        &tenant,
        &json!({"name": "slow", "kind": "echo", "spec": {"schema": {"type": "object"}}}),
    )
    .await
    .expect("publish");

    let set = mcphost::triggers::set(
        &state,
        &tenant,
        &json!({"tool": "slow", "schedule": "* * * * *"}),
    )
    .await
    .expect("trigger.set");
    let trigger_id = set["id"].as_str().expect("id").to_string();

    // Fabricate "the previous firing is still running": a `queued` run
    // already attached to this trigger, never leased/finalized (no
    // executor is running in this bare `AppState` to touch it).
    let still_running_run_id = mcphost::state::new_ulid();
    state
        .db
        .insert_queued_run(
            still_running_run_id.clone(),
            tenant.id,
            "slow".to_string(),
            "schedule".to_string(),
            Some(trigger_id.clone()),
            None,
            900,
            "{}".to_string(),
            false,
            false,
        )
        .await
        .expect("insert still-running run");

    let now = mcphost::state::now_unix();
    state
        .db
        .update_trigger_after_fire(trigger_id.clone(), Some(now), None, 0)
        .await
        .expect("force due");
    mcphost::triggers::tick_once(&state).await.expect("tick");

    let runs = state
        .db
        .list_runs(tenant.id, None, Some("skipped".to_string()), Some("schedule".to_string()), 20)
        .await
        .expect("list_runs");
    let skipped_overlap = runs
        .iter()
        .any(|r| r.trigger_ref.as_deref() == Some(trigger_id.as_str()) && r.error_class.as_deref() == Some("overlap"));
    assert!(
        skipped_overlap,
        "expected a skipped/overlap run for trigger {trigger_id}: {runs:?}"
    );

    // The manually-inserted "still running" run is itself untouched --
    // the skip must not have finalized it.
    let still_running = state
        .db
        .get_run(still_running_run_id, tenant.id)
        .await
        .expect("get_run")
        .expect("run exists");
    assert_eq!(still_running.status, "queued");
}
