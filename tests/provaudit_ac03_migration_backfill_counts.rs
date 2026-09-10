//! PRD-mcphost-provenance-audit
//! AC3 — Given the migration on a copy of the live DB, When it completes,
//! Then zero rows have NULL origin and the journaled class counts sum to
//! the row total.
//!
//! Exercises `Db::backfill_provenance` directly (the exact function
//! `migrate_0012_provenance`'s one-shot gate calls) against fixture rows
//! seeded with `origin: 'unclassified'` -- the same state a genuinely
//! pre-0012 row is in the instant migration 0012's `ALTER TABLE`s land.
//! `attrib_ac3_backfill_migration.rs` established this pattern for
//! migration 0010.

mod common;
use common::TempDataDir;
use mcphost::auth::{generate_key, generate_namespace, hash_key};
use mcphost::db::Db;
use rusqlite::params;

#[tokio::test]
async fn backfill_provenance_classifies_every_unclassified_row_exactly_once() {
    let dir = TempDataDir::new();
    let db = Db::open(&dir.0).expect("open db");
    db.migrate().await.expect("migrate");
    let db_path = dir.0.join("mcphost.db");

    // Four tenants, seeded pre-0012-style: `origin: "unclassified"` with a
    // mix of `source_class`/`synthetic` combinations.
    let mut tenant_ids = Vec::new();
    for (display_name, source_class, synthetic) in [
        ("Real Caller".to_string(), Some("external".to_string()), None),
        ("Loopback Persona".to_string(), Some("loopback".to_string()), Some("harness:unstamped".to_string())),
        ("wintermute-hub".to_string(), Some("fleet".to_string()), None),
        ("Unclassified Legacy".to_string(), None, None),
    ] {
        let key = generate_key();
        let namespace = generate_namespace();
        let tenant = db
            .create_tenant_attributed(
                display_name,
                namespace,
                hash_key(&key),
                synthetic,
                source_class,
                None,
                None,
                "unclassified".to_string(),
                None,
            )
            .await
            .expect("seed tenant");
        tenant_ids.push(tenant.id);
    }
    // Expected buckets: tenant[0] (external, no synthetic) -> external;
    // tenant[1] (loopback, synthetic set) -> synthetic; tenant[2] (fleet)
    // -> synthetic; tenant[3] (no source_class, no synthetic) -> external.

    // Three signup_events rows, seeded the same pre-0012 way:
    // `record_signup_event_attributed`'s INSERT never names `origin`, so
    // SQLite's column default (`'unclassified'`) applies automatically --
    // exactly the state a genuinely pre-0012 row would be in.
    db.record_signup_event_attributed("127.0.0.1".to_string(), None, None)
        .await
        .expect("seed signup loopback"); // -> synthetic (loopback IP)
    db.record_signup_event_attributed(
        "203.0.113.9".to_string(),
        Some("harness:unstamped".to_string()),
        None,
    )
    .await
    .expect("seed signup labeled"); // -> synthetic (explicit label)
    db.record_signup_event_attributed("203.0.113.10".to_string(), None, None)
        .await
        .expect("seed signup external"); // -> external

    // Two calls rows, seeded via a raw INSERT that omits `origin` (same
    // reasoning as the signup_events seeds above) -- one against the
    // external tenant, one against a synthetic tenant.
    {
        let conn = rusqlite::Connection::open(&db_path).expect("open raw db");
        for tenant_id in [tenant_ids[0], tenant_ids[1]] {
            conn.execute(
                "INSERT INTO calls (tenant_id, tool_name, started_at, started_unix, duration_ms, ok) \
                 VALUES (?1, 'seed_tool', 'unix:0.0', 0, 1, 1)",
                params![tenant_id],
            )
            .expect("seed call row");
        }
    }

    let counts = db.backfill_provenance().await.expect("backfill");
    assert_eq!(counts.tenants_external, 2, "{counts:?}");
    assert_eq!(counts.tenants_synthetic, 2, "{counts:?}");
    assert_eq!(counts.signup_events_external, 1, "{counts:?}");
    assert_eq!(counts.signup_events_synthetic, 2, "{counts:?}");
    assert_eq!(counts.calls_external, 1, "{counts:?}");
    assert_eq!(counts.calls_synthetic, 1, "{counts:?}");

    // Every seeded row total is accounted for by the sum of its two
    // buckets.
    assert_eq!(counts.tenants_external + counts.tenants_synthetic, 4);
    assert_eq!(counts.signup_events_external + counts.signup_events_synthetic, 3);
    assert_eq!(counts.calls_external + counts.calls_synthetic, 2);

    // Zero rows anywhere still `origin = 'unclassified'`.
    let conn = rusqlite::Connection::open(&db_path).expect("open raw db");
    for table in ["tenants", "signup_events", "calls"] {
        let remaining: i64 = conn
            .query_row(
                &format!("SELECT COUNT(*) FROM {table} WHERE origin = 'unclassified'"),
                [],
                |r| r.get(0),
            )
            .expect("count unclassified");
        assert_eq!(remaining, 0, "table {table} still has unclassified rows");
    }
    drop(conn);

    // Calling the backfill again is a no-op: nothing is left unclassified.
    let counts_again = db.backfill_provenance().await.expect("re-run backfill");
    assert_eq!(counts_again, mcphost::db::ProvenanceBackfillCounts::default());
}
