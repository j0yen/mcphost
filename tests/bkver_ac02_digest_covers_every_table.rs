//! PRD-mcphost-backup-restore-proof-nightly
//! AC2 — Given a freshly migrated db, When the digest runs, Then every
//! table in `sqlite_master` appears in `tables` or in `DIGEST_EXCLUDE`, and
//! a test adding a fixture table to neither fails.

use crate::common;
use common::TempDataDir;
use mcphost::backup::{digest_db, master_tables, uncovered_tables, DIGEST_EXCLUDE};
use mcphost::db::Db;
use std::collections::BTreeSet;
use std::path::Path;

fn digested_names(report: &serde_json::Value) -> BTreeSet<String> {
    report["tables"]
        .as_object()
        .expect("tables object")
        .keys()
        .cloned()
        .collect()
}

fn master_of(path: &Path) -> Vec<String> {
    let conn = rusqlite::Connection::open(path).expect("open");
    master_tables(&conn).expect("master tables")
}

#[tokio::test]
async fn bkver_ac02() {
    let dir = TempDataDir::new();
    let db = Db::open(&dir.0).expect("open db");
    db.migrate().await.expect("migrate");
    let path = dir.0.join("mcphost.db");

    let report = digest_db(&path, 1_800_000_000);
    assert!(report.integrity_ok, "fresh db passes integrity: {}", report.value);
    assert_eq!(report.value["integrity"], "ok");
    let digested = digested_names(&report.value);
    let master = master_of(&path);
    assert!(master.len() > 50, "a migrated db has many tables, saw {}", master.len());

    // Every table is digested or explicitly excluded, and the exclude list
    // only names tables that exist (a stale name would hide nothing).
    assert_eq!(uncovered_tables(&master, &digested, DIGEST_EXCLUDE), Vec::<String>::new());
    for ex in DIGEST_EXCLUDE {
        assert!(master.iter().any(|t| t == ex), "DIGEST_EXCLUDE names missing table {ex}");
        assert!(!digested.contains(*ex), "{ex} is excluded yet digested");
    }

    // A fixture table added to a copy is digested automatically (the set is
    // generated from sqlite_master), so it is covered ...
    {
        let conn = rusqlite::Connection::open(&path).expect("open rw");
        conn.execute_batch("CREATE TABLE bkver_fixture (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO bkver_fixture(v) VALUES ('x');")
            .expect("fixture table");
    }
    let report2 = digest_db(&path, 1_800_000_000);
    let digested2 = digested_names(&report2.value);
    assert!(digested2.contains("bkver_fixture"));
    assert_ne!(report.value["digest"], report2.value["digest"]);
    let master2 = master_of(&path);
    assert!(uncovered_tables(&master2, &digested2, DIGEST_EXCLUDE).is_empty());

    // ... and the drift check FAILS for a table in neither set: the digest
    // as it stood before the fixture table, against the master that has it.
    let drift = uncovered_tables(&master2, &digested, DIGEST_EXCLUDE);
    assert_eq!(drift, vec!["bkver_fixture".to_string()]);
}

#[test]
fn bkver_ac02_schema_version_is_the_migration_count() {
    let max = std::fs::read_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/migrations"))
        .expect("migrations dir")
        .filter_map(|e| e.ok()?.file_name().into_string().ok())
        .filter_map(|n| n.get(..4)?.parse::<u32>().ok())
        .max()
        .expect("a migration");
    assert_eq!(mcphost::backup::SCHEMA_VERSION, max);
}
