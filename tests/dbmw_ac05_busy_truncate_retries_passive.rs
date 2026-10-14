//! PRD-mcphost-db-maintenance-window AC5 (P0): with a reader holding a long
//! transaction `wal_checkpoint(TRUNCATE)` reports busy; the executor retries
//! PASSIVE once, the row reads `ok` with `checkpoint_result: PASSIVE` and no
//! statement ran past its 30-s deadline.

use crate::common;

use mcphost::heal::HealOutcome;
use rusqlite::Connection;

#[tokio::test]
async fn busy_truncate_falls_back_to_one_passive_checkpoint() {
    let (state, dir) = common::bare_app_state().await;
    let path = dir.0.join("mcphost.db");

    // The reader pins a snapshot, then a writer appends WAL frames past it,
    // so TRUNCATE cannot finish while the reader's transaction is open.
    let reader = Connection::open(&path).expect("reader");
    reader.execute_batch("BEGIN;").unwrap();
    let _: i64 = reader.query_row("SELECT count(*) FROM sqlite_master", [], |r| r.get(0)).unwrap();
    let writer = Connection::open(&path).expect("writer");
    writer.pragma_update(None, "wal_autocheckpoint", 0).unwrap();
    writer.pragma_update(None, "synchronous", "OFF").unwrap();
    writer
        .execute_batch(
            "BEGIN; CREATE TABLE dbmw_busy (id INTEGER PRIMARY KEY, v BLOB); \
             WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 50) \
             INSERT INTO dbmw_busy SELECT i, zeroblob(2000) FROM n; COMMIT;",
        )
        .unwrap();

    let row = mcphost::maintenance::db_maintenance(&state.db, mcphost::state::now_unix(), false)
        .await
        .expect("db_maintenance");
    assert_eq!(row.outcome, HealOutcome::Ok, "{row:?}");
    assert_eq!(row.after["checkpoint_result"], "PASSIVE", "{}", row.after);
    assert!(row.duration_ms < 30_000, "statement deadline: {} ms", row.duration_ms);
    drop(reader);
}
