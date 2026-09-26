//! PRD-mcphost-runs-end-user-subject
//! AC6 (P0) — Given a tenant with 100k runs, When migration 0048 applies in
//! the test harness, Then it completes under 1 s and `PRAGMA
//! index_list(runs)` shows the new index.
//!
//! Migration 0048 is additive (`ALTER TABLE ADD COLUMN` x3 plus one
//! `CREATE INDEX`) and every migration is gated on its own already-applied
//! signal, so the way to time JUST 0048's real cost against a pre-existing
//! 100k-row `runs` table is: migrate a fresh db (cheap, empty tables),
//! populate `runs` with 100k rows, undo 0048's own effect (drop its three
//! columns and its index -- everything else stays migrated), then migrate
//! again -- every other migration's gate short-circuits as a no-op, so the
//! measured wall time is 0048's `ALTER TABLE`s/`CREATE INDEX` alone,
//! exactly the operation a genuinely pre-0048 database would run once.

use crate::common;
use common::TempDataDir;
use mcphost::db::Db;
use rusqlite::params;
use std::time::Instant;

#[tokio::test]
async fn migration_0048_completes_under_1s_at_100k_rows_and_adds_its_index() {
    let dir = TempDataDir::new();
    let db = Db::open(&dir.0).expect("open db");
    db.migrate().await.expect("initial migrate");
    let db_path = dir.0.join("mcphost.db");

    let tenant = db
        .create_tenant(
            "AC6 Tenant".to_string(),
            mcphost::auth::generate_namespace(),
            mcphost::auth::hash_key(&mcphost::auth::generate_key()),
            None,
        )
        .await
        .expect("seed tenant");

    {
        let mut conn = rusqlite::Connection::open(&db_path).expect("open raw db");
        conn.execute_batch(
            "DROP INDEX idx_runs_tenant_end_user; \
             ALTER TABLE runs DROP COLUMN end_user_subject; \
             ALTER TABLE runs DROP COLUMN end_user_issuer; \
             ALTER TABLE runs DROP COLUMN end_user_method;",
        )
        .expect("undo migration 0048");

        let tx = conn.transaction().expect("begin seed tx");
        for i in 0..100_000i64 {
            tx.execute(
                "INSERT INTO runs (id, tenant_id, tool_name, trigger, status, attempt) \
                 VALUES (?1, ?2, 'seed_tool', 'job', 'done', 1)",
                params![format!("01AC6SEED{i:07}"), tenant.id],
            )
            .expect("seed run row");
        }
        tx.commit().expect("commit seed tx");
    }

    let started = Instant::now();
    db.migrate().await.expect("re-migrate applies 0048 against 100k rows");
    let elapsed = started.elapsed();
    assert!(
        elapsed.as_secs_f64() < 1.0,
        "migration 0048 must complete under 1s at 100k rows, took {elapsed:?}"
    );

    let conn = rusqlite::Connection::open(&db_path).expect("open raw db");
    let mut stmt = conn.prepare("PRAGMA index_list(runs)").expect("prepare index_list");
    let index_names: Vec<String> = stmt
        .query_map([], |r| r.get::<_, String>(1))
        .expect("query index_list")
        .collect::<Result<Vec<_>, _>>()
        .expect("collect index names");
    assert!(
        index_names.iter().any(|n| n == "idx_runs_tenant_end_user"),
        "expected idx_runs_tenant_end_user in PRAGMA index_list(runs): {index_names:?}"
    );
}
