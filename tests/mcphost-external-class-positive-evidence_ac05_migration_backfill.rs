//! PRD-mcphost-external-class-positive-evidence
//! AC5 (P0) — Given one seeded tenant per case (external+verified,
//! external+later-day call only, external+same-day only, unclassified,
//! synthetic+verified), When the migration runs, Then the verified row is
//! `external`/`claimed-email`, both unverified external rows are
//! `unattributed`/`legacy-external-unverified`, the unclassified row is
//! `unattributed`/`legacy-unclassified`, the synthetic row is unchanged, no
//! row is NULL, and the journaled before and after counts sum to the same
//! total.
//!
//! The migration is gated on the absence of `tenants.origin_tainted_unix`,
//! so the fixture migrates a fresh DB, rolls that one migration back by hand
//! (drops its column and tables), seeds the pre-0137 rows, and migrates again.

use crate::common;
use common::TempDataDir;
use mcphost::auth::{generate_key, generate_namespace, hash_key};
use mcphost::db::Db;
use rusqlite::{Connection, params};

const DAY: i64 = 86_400;

async fn seed(db: &Db, name: &str, origin: &str, detail: Option<&str>) -> i64 {
    db.create_tenant_attributed(
        name.to_string(),
        generate_namespace(),
        hash_key(&generate_key()),
        None,
        Some("external".to_string()),
        None,
        None,
        origin.to_string(),
        detail.map(str::to_string),
    )
    .await
    .expect("seed tenant")
    .id
}

fn row(conn: &Connection, id: i64) -> (String, Option<String>) {
    conn.query_row("SELECT origin, origin_detail FROM tenants WHERE id = ?1", params![id], |r| {
        Ok((r.get(0)?, r.get(1)?))
    })
    .unwrap()
}

fn journal_total(conn: &Connection, phase: &str, table: &str) -> i64 {
    conn.query_row(
        "SELECT COALESCE(SUM(n), 0) FROM origin_migration_journal WHERE phase = ?1 AND table_name = ?2",
        params![phase, table],
        |r| r.get(0),
    )
    .unwrap()
}

#[tokio::test]
async fn mcphost_external_class_positive_evidence_ac05_migration_relabels_each_case() {
    let dir = TempDataDir::new();
    let db = Db::open(&dir.0).expect("open db");
    db.migrate().await.expect("migrate");
    let path = dir.0.join("mcphost.db");
    let conn = Connection::open(&path).expect("raw conn");

    // Roll the 0137 migration back to the state prod is in before it.
    conn.execute_batch(
        "DROP TABLE origin_migration_journal; DROP TABLE origin_counters; \
         ALTER TABLE tenants DROP COLUMN origin_tainted_unix;",
    )
    .expect("roll back 0137");

    let ext = Some("external-unverified");
    let verified = seed(&db, "ext verified", "external", ext).await;
    let later_day = seed(&db, "ext later day", "external", ext).await;
    let same_day = seed(&db, "ext same day", "external", ext).await;
    let unclassified = seed(&db, "unclassified", "unclassified", None).await;
    let synthetic = seed(&db, "synthetic verified", "synthetic", Some("harness:unstamped")).await;
    let internal = seed(&db, "perf-free", "internal", Some("perf-free")).await;
    let created = 1_700_000_000_i64;
    for (id, last_seen, verified_at) in [
        (verified, created + 60, Some(created + 120)),
        (later_day, created + 3 * DAY, None),
        (same_day, created + 60, None),
        (unclassified, created + 60, None),
        (synthetic, created + 60, Some(created + 120)),
        (internal, created + 60, None),
    ] {
        conn.execute(
            "UPDATE tenants SET created_unix = ?2, last_seen_unix = ?3, owner_verified_at = ?4 WHERE id = ?1",
            params![id, created, last_seen, verified_at],
        )
        .unwrap();
    }
    // signup_events: two external, one unclassified, one synthetic.
    for (ip, origin) in [("203.0.113.1", "external"), ("203.0.113.2", "external"), ("203.0.113.3", "unclassified"), ("127.0.0.1", "synthetic")] {
        conn.execute(
            "INSERT INTO signup_events (source_ip, created_unix, origin, origin_detail) VALUES (?1, ?2, ?3, 'seed')",
            params![ip, created, origin],
        )
        .unwrap();
    }

    db.migrate().await.expect("migrate again: runs 0137");

    assert_eq!(row(&conn, verified), ("external".to_string(), Some("claimed-email".to_string())));
    for id in [later_day, same_day] {
        assert_eq!(
            row(&conn, id),
            ("unattributed".to_string(), Some("legacy-external-unverified".to_string()))
        );
    }
    assert_eq!(
        row(&conn, unclassified),
        ("unattributed".to_string(), Some("legacy-unclassified".to_string()))
    );
    assert_eq!(row(&conn, synthetic), ("synthetic".to_string(), Some("harness:unstamped".to_string())));
    assert_eq!(row(&conn, internal), ("internal".to_string(), Some("perf-free".to_string())));

    let null_rows: i64 = conn
        .query_row(
            "SELECT (SELECT COUNT(*) FROM tenants WHERE origin IS NULL OR origin = 'unclassified') \
                  + (SELECT COUNT(*) FROM signup_events WHERE origin IS NULL OR origin = 'unclassified')",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(null_rows, 0);

    let se: Vec<(String, i64)> = {
        let mut stmt = conn.prepare("SELECT origin, COUNT(*) FROM signup_events GROUP BY origin ORDER BY origin").unwrap();
        stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?))).unwrap().map(Result::unwrap).collect()
    };
    assert_eq!(se, vec![("synthetic".to_string(), 1), ("unattributed".to_string(), 3)]);

    // Journal: before and after sum to the same total, per table and overall.
    let tenants_total: i64 = conn.query_row("SELECT COUNT(*) FROM tenants", [], |r| r.get(0)).unwrap();
    assert_eq!(journal_total(&conn, "before", "tenants"), tenants_total);
    assert_eq!(journal_total(&conn, "after", "tenants"), tenants_total);
    assert_eq!(journal_total(&conn, "before", "signup_events"), 4);
    assert_eq!(journal_total(&conn, "after", "signup_events"), 4);
    let before_ext: i64 = conn
        .query_row(
            "SELECT n FROM origin_migration_journal WHERE phase='before' AND table_name='tenants' AND origin='external'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(before_ext, 3);

    // The gate: a third run does not re-label (a promoted row stays promoted).
    conn.execute("UPDATE tenants SET origin = 'external', origin_detail = 'returned:2d' WHERE id = ?1", params![later_day])
        .unwrap();
    db.migrate().await.expect("migrate a third time");
    assert_eq!(row(&conn, later_day), ("external".to_string(), Some("returned:2d".to_string())));
}
