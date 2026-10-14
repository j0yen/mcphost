//! PRD-mcphost-db-maintenance-window AC2 (P0): a database with a 3 MB WAL and
//! 1 000 freelist pages under `auto_vacuum = INCREMENTAL` comes out of
//! `DbMaintenance` with an empty WAL, an empty freelist, `checkpoint_result:
//! TRUNCATE`, `outcome: ok` and a positive `optimize_ms`.

use crate::common;

use mcphost::heal::HealOutcome;
use rusqlite::Connection;

/// Builds ~1 000 freelist pages and a ~3 MB WAL on `path`, holding the WAL
/// open (the caller's `Db` keeps its own connection alive, so the file stays).
pub fn seed_wal_and_freelist(path: &std::path::Path) {
    let conn = Connection::open(path).expect("open raw");
    conn.pragma_update(None, "wal_autocheckpoint", 0).expect("autocheckpoint off");
    conn.execute_batch("CREATE TABLE dbmw_pad (id INTEGER PRIMARY KEY, v BLOB); CREATE TABLE dbmw_junk (id INTEGER PRIMARY KEY, v BLOB);")
        .expect("create");
    conn.execute_batch("BEGIN;").unwrap();
    for i in 0..1100 {
        conn.execute("INSERT INTO dbmw_junk (id, v) VALUES (?1, zeroblob(3500))", [i]).unwrap();
    }
    for i in 0..800 {
        conn.execute("INSERT INTO dbmw_pad (id, v) VALUES (?1, zeroblob(3500))", [i]).unwrap();
    }
    conn.execute_batch("COMMIT;").unwrap();
    conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(())).unwrap();
    conn.execute("DELETE FROM dbmw_junk WHERE id >= 0", []).unwrap();
    conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(())).unwrap();
    conn.execute("UPDATE dbmw_pad SET v = zeroblob(3499)", []).unwrap();
}

#[tokio::test]
async fn db_maintenance_empties_wal_and_freelist() {
    let (state, dir) = common::bare_app_state().await;
    let db_path = dir.0.join("mcphost.db");
    seed_wal_and_freelist(&db_path);

    let av: i64 = Connection::open(&db_path).unwrap().query_row("PRAGMA auto_vacuum", [], |r| r.get(0)).unwrap();
    assert_eq!(av, 2, "fixture database is INCREMENTAL");
    let wal_bytes = std::fs::metadata(dir.0.join("mcphost.db-wal")).expect("wal exists").len();
    assert!(wal_bytes >= 3 * 1024 * 1024, "wal is {wal_bytes} bytes");

    let row = mcphost::maintenance::db_maintenance(&state.db, mcphost::state::now_unix(), false)
        .await
        .expect("db_maintenance");
    assert_eq!(row.outcome, HealOutcome::Ok, "{row:?}");
    assert!(row.before["freelist_count"].as_i64().unwrap() >= 1000, "{}", row.before);
    assert!(row.before["wal_mb"].as_f64().unwrap() >= 3.0, "{}", row.before);
    assert_eq!(row.after["wal_mb"].as_f64(), Some(0.0), "{}", row.after);
    assert_eq!(row.after["freelist_count"].as_i64(), Some(0), "{}", row.after);
    assert_eq!(row.after["checkpoint_result"], "TRUNCATE");
    assert!(row.after["optimize_ms"].as_i64().unwrap() > 0, "{}", row.after);

    let stored = state.db.find_heal_row(row.id).await.unwrap().expect("ledger row");
    assert_eq!(stored.outcome, HealOutcome::Ok);
}
