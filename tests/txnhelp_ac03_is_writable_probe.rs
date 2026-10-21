//! PRD-mcphost-transactions-only-via-helper AC3 -- the writability probe
//! (`db::is_writable`) still takes and releases the write lock through
//! `with_tx(Immediate)`: busy under another writer, writable and uncommitted
//! otherwise, and an `Err` rather than a false "writable" with a transaction
//! already open.

use mcphost::db::is_writable;
use rusqlite::Connection;
use std::time::Duration;

#[test]
fn txnhelp_ac03() {
    let path = std::env::temp_dir().join(format!(
        "txnhelp-ac03-{}-{}.db",
        std::process::id(),
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
    ));
    let probe = Connection::open(&path).unwrap();
    probe.execute_batch("CREATE TABLE t (v TEXT)").unwrap();
    probe.busy_timeout(Duration::from_millis(50)).unwrap();
    let other = Connection::open(&path).unwrap();

    // A second connection holds the write lock: not writable, and fast.
    other.execute_batch("BEGIN IMMEDIATE").unwrap();
    let held = is_writable(&probe).expect("a busy database is reported, not an error");
    assert!(!held, "a held write lock means not writable");
    // ... and gives up inside 1 s (50 ms busy_timeout), warm-median budget.
    crate::perf_budget!(1000, {
        assert!(!is_writable(&probe).expect("busy, not an error"));
    });
    assert!(probe.is_autocommit(), "the failed probe leaves autocommit");
    other.execute_batch("ROLLBACK").unwrap();

    // No other writer: writable, nothing committed, back in autocommit, and
    // the lock the probe took is released again.
    let before: i64 = probe.query_row("PRAGMA data_version", [], |r| r.get(0)).unwrap();
    assert!(is_writable(&probe).expect("probe"), "no other writer means writable");
    assert!(probe.is_autocommit(), "the probe connection is back in autocommit");
    let rows: i64 = other.query_row("SELECT COUNT(*) FROM t", [], |r| r.get(0)).unwrap();
    assert_eq!(rows, 0, "the probe commits nothing");
    other.execute_batch("BEGIN IMMEDIATE; ROLLBACK").expect("the probe released its write lock");
    let after: i64 = probe.query_row("PRAGMA data_version", [], |r| r.get(0)).unwrap();
    assert_eq!(before, after, "the probe wrote nothing another connection could see");

    // A transaction already open: `with_tx` would take the nested SAVEPOINT
    // path, take no lock, and say "writable" whatever the lock state.
    other.execute_batch("BEGIN IMMEDIATE").unwrap();
    probe.execute_batch("BEGIN").unwrap();
    let inside = is_writable(&probe);
    assert!(inside.is_err(), "inside an open transaction the probe refuses instead of answering: {inside:?}");
    assert!(!probe.is_autocommit(), "the caller's transaction is left open and untouched");
    probe.execute_batch("ROLLBACK").unwrap();
    other.execute_batch("ROLLBACK").unwrap();
    let _ = std::fs::remove_file(&path);
}
