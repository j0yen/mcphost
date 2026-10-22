//! PRD-mcphost-external-class-positive-evidence
//! AC5 — Given one seeded tenant per case (external+verified, external+
//! later-day call only, external+same-day only, unclassified,
//! synthetic+verified), When the migration runs, Then the verified row is
//! `external`/`claimed-email`, both unverified external rows are
//! `unattributed`/`legacy-external-unverified`, the unclassified row is
//! `unattributed`/`legacy-unclassified`, the synthetic row is unchanged, no
//! row is NULL, and the journaled before and after counts sum to the same
//! total.

use crate::common;
use common::TempDataDir;
use rusqlite::{Connection, params};

async fn seed(db: &mcphost::db::Db, name: &str, origin: &str, detail: &str) -> i64 {
    let key = mcphost::auth::generate_key();
    db.create_tenant_attributed(
        name.to_string(),
        mcphost::auth::generate_namespace(),
        mcphost::auth::hash_key(&key),
        None,
        None,
        None,
        None,
        origin.to_string(),
        Some(detail.to_string()),
    )
    .await
    .expect("seed tenant")
    .id
}

fn row(conn: &Connection, id: i64) -> (String, Option<String>) {
    conn.query_row("SELECT origin, origin_detail FROM tenants WHERE id = ?1", params![id], |r| {
        Ok((r.get(0)?, r.get(1)?))
    })
    .expect("row")
}

fn journal_total(conn: &Connection, phase: &str, tbl: &str) -> i64 {
    conn.query_row(
        "SELECT COALESCE(SUM(n), 0) FROM origin_tristate_journal WHERE phase = ?1 AND tbl = ?2",
        params![phase, tbl],
        |r| r.get(0),
    )
    .expect("journal sum")
}

#[tokio::test]
async fn mcphost_external_class_positive_evidence_ac05() {
    let dir = TempDataDir::new();
    let db = mcphost::db::Db::open(&dir.0).expect("open db");
    db.migrate().await.expect("migrate");
    let conn = Connection::open(dir.0.join("mcphost.db")).expect("raw db");

    let verified = seed(&db, "ext verified", "external", "external-unverified").await;
    let later_day = seed(&db, "ext later day", "external", "external-unverified").await;
    let same_day = seed(&db, "ext same day", "external", "external-unverified").await;
    let unclassified = seed(&db, "unclassified", "unclassified", "x").await;
    let synthetic = seed(&db, "synthetic verified", "synthetic", "harness:unstamped").await;
    let internal = seed(&db, "perf", "internal", "perf-free").await;
    conn.execute("UPDATE tenants SET owner_verified_at = 1000 WHERE id IN (?1, ?2)", params![verified, synthetic])
        .unwrap();
    conn.execute("UPDATE tenants SET created_unix = 1000, last_seen_unix = 1000 + 2 * 86400 WHERE id = ?1", params![later_day])
        .unwrap();
    conn.execute("UPDATE tenants SET created_unix = 1000, last_seen_unix = 1000 + 3600 WHERE id = ?1", params![same_day])
        .unwrap();
    for (ip, origin) in [("8.8.8.8", "external"), ("8.8.4.4", "unclassified"), ("127.0.0.1", "synthetic")] {
        conn.execute(
            "INSERT INTO signup_events (source_ip, created_unix, origin) VALUES (?1, 1, ?2)",
            params![ip, origin],
        )
        .unwrap();
    }

    // Re-arm the one-shot gate (the journal table is the marker) and migrate.
    conn.execute_batch("DROP TABLE origin_tristate_journal").unwrap();
    db.migrate().await.expect("re-migrate");

    assert_eq!(row(&conn, verified), ("external".into(), Some("claimed-email".into())));
    for id in [later_day, same_day] {
        assert_eq!(row(&conn, id), ("unattributed".into(), Some("legacy-external-unverified".into())));
    }
    assert_eq!(row(&conn, unclassified), ("unattributed".into(), Some("legacy-unclassified".into())));
    assert_eq!(row(&conn, synthetic), ("synthetic".into(), Some("harness:unstamped".into())));
    assert_eq!(row(&conn, internal), ("internal".into(), Some("perf-free".into())));
    let nulls: i64 = conn
        .query_row("SELECT COUNT(*) FROM tenants WHERE origin IS NULL OR origin = ''", [], |r| r.get(0))
        .unwrap();
    assert_eq!(nulls, 0);

    for tbl in ["tenants", "signup_events"] {
        let (before, after) = (journal_total(&conn, "before", tbl), journal_total(&conn, "after", tbl));
        let live: i64 = conn.query_row(&format!("SELECT COUNT(*) FROM {tbl}"), [], |r| r.get(0)).unwrap();
        assert_eq!(before, after, "{tbl}: journaled totals must match");
        assert_eq!(after, live, "{tbl}: journal total is the table's row count");
    }
    let signup_unattributed: i64 = conn
        .query_row("SELECT COUNT(*) FROM signup_events WHERE origin = 'unattributed'", [], |r| r.get(0))
        .unwrap();
    assert_eq!(signup_unattributed, 2);

    // The gate holds: a promotion earned after the migration survives the
    // next startup's migrate().
    conn.execute("UPDATE tenants SET origin = 'external', origin_detail = 'returned:2d' WHERE id = ?1", params![later_day])
        .unwrap();
    db.migrate().await.expect("migrate again");
    assert_eq!(row(&conn, later_day), ("external".into(), Some("returned:2d".into())));
}
