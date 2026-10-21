//! PRD-mcphost-transactions-only-via-helper AC4 -- migration 0116 applies
//! all-or-nothing through `with_tx(Deferred)`. (The repo carries no
//! `PRAGMA user_version`: a migration's "version" is whether its tables
//! exist, and `migrate_0116_orgs` is gated on the `orgs` table.)

use mcphost::db::Db;
use rusqlite::Connection;

fn exists(conn: &Connection, name: &str) -> bool {
    conn.query_row("SELECT COUNT(*) FROM sqlite_master WHERE name = ?1", [name], |r| r.get::<_, i64>(0)).unwrap() > 0
}

fn user_version(conn: &Connection) -> i64 {
    conn.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap()
}

#[test]
fn txnhelp_ac04() {
    let dir = std::env::temp_dir().join(format!(
        "txnhelp-ac04-{}-{}",
        std::process::id(),
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
    ));
    // A fully migrated database, then rewound to the state before 0116.
    drop(Db::open(&dir).expect("fresh open migrates"));
    let path = dir.join("mcphost.db");
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch("DROP TABLE org_members; DROP TABLE orgs;").unwrap();
    assert!(!exists(&conn, "orgs") && !exists(&conn, "org_members"));
    let version_before = user_version(&conn);

    // Happy path: 0116's tables exist afterwards, the connection is in autocommit.
    Db::migrate_0116_orgs(&conn).expect("0116 applies");
    assert!(exists(&conn, "orgs") && exists(&conn, "org_members"), "0116 tables exist");
    assert_eq!(user_version(&conn), version_before, "the migration does not touch user_version");
    assert!(conn.is_autocommit(), "autocommit after a successful 0116");

    // Failure path: an index named like 0116's second CREATE (`org_members`)
    // makes that statement fail mid-batch, after `orgs` was created.
    conn.execute_batch("DROP TABLE org_members; DROP TABLE orgs; CREATE INDEX org_members ON tenants(id);").unwrap();
    assert!(!exists(&conn, "orgs"));
    let failed = Db::migrate_0116_orgs(&conn);
    assert!(failed.is_err(), "0116 fails on its second CREATE");
    assert!(!exists(&conn, "orgs"), "the table from 0116's first CREATE was rolled back");
    assert_eq!(user_version(&conn), version_before, "the schema version has not advanced");
    assert!(conn.is_autocommit(), "autocommit after a failed 0116");
    let fk: i64 = conn.query_row("PRAGMA foreign_keys", [], |r| r.get(0)).unwrap();
    assert_eq!(fk, 1, "foreign_keys is switched back on after a failed batch");

    // The whole `migrate_sync` run hits the same failure and refuses to open.
    drop(conn);
    assert!(Db::open(&dir).is_err(), "migrate_sync fails on the broken 0116 batch");
    let after = Connection::open(&path).unwrap();
    assert!(!exists(&after, "orgs"), "no partial 0116 left behind by migrate_sync");
    let _ = std::fs::remove_dir_all(&dir);
}
