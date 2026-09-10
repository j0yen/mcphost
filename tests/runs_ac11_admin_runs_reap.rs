//! AC11 (P1) — Given a run left `running` past its deadline by a killed
//! executor, When `admin.runs_reap` runs, Then the run reads `error` with
//! `error_class: interrupted`.
//!
//! Deliberately builds a bare `AppState` directly (no HTTP listener, no
//! `runs::spawn_executor`) rather than `common::TestServer` -- the live
//! background executor `TestServer` normally runs would otherwise race
//! this test's own manual lease-and-strand of a `queued` run (whichever
//! side wins the lease first is nondeterministic under load), and even a
//! losing race would still eventually finalize the row through the
//! executor's own dispatch path, which is exactly the "crashed, never
//! finalized" scenario this test needs to fabricate deterministically
//! instead.

mod common;
use mcphost::db::Db;
use mcphost::kinds::KindRegistry;
use mcphost::secrets::SecretBox;
use mcphost::state::AppState;
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
    };
    (state, data_dir)
}

#[tokio::test]
async fn admin_runs_reap_marks_expired_running_runs_interrupted() {
    let (state, _data_dir) = bare_state().await;

    let tenant = state
        .db
        .create_tenant(
            "AC11 Tenant".to_string(),
            "t-reap".to_string(),
            "key-hash-reap".to_string(),
            None,
        )
        .await
        .expect("create_tenant");

    let run_id = mcphost::state::new_ulid();
    state
        .db
        .insert_queued_run(
            run_id.clone(),
            tenant.id,
            "ghost_tool".to_string(),
            "job".to_string(),
            None,
            None,
            1, // deadline_s: expires almost immediately
            "{}".to_string(),
        )
        .await
        .expect("insert_queued_run");

    let leased = state
        .db
        .lease_next_queued_run(tenant.id)
        .await
        .expect("lease_next_queued_run")
        .expect("a queued run must be leaseable -- no background executor is running here");
    assert_eq!(leased.id, run_id);
    assert_eq!(leased.status, "running");

    // `reap_expired_runs` compares whole-second unix timestamps
    // (`started_unix + deadline_s < now`); sleeping comfortably past two
    // full seconds avoids a boundary flake where a sub-second sleep lands
    // on the same truncated second as `started_unix + deadline_s`.
    tokio::time::sleep(std::time::Duration::from_millis(2500)).await;

    let reap = mcphost::runs::admin_runs_reap(&state)
        .await
        .expect("admin_runs_reap ok");
    assert!(
        reap["reaped"].as_i64().unwrap_or(0) >= 1,
        "at least this run must be reaped: {reap}"
    );

    let after = state
        .db
        .get_run(run_id, tenant.id)
        .await
        .expect("get_run")
        .expect("run still exists");
    assert_eq!(after.status, "error");
    assert_eq!(after.error_class.as_deref(), Some("interrupted"));
}
