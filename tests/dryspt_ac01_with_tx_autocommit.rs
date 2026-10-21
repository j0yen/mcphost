//! PRD-mcphost-dry-run-inside-savepoint-for-transactional-verbs
//! AC1 -- `with_tx(Immediate)` on a file-backed autocommit connection holds
//! the write lock while the closure runs, commits on `Ok`, rolls back on
//! `Err` and on a panic, and leaves the connection in autocommit.

use mcphost::errors::AppError;
use mcphost::txn::{with_tx, TxBehavior};
use rusqlite::Connection;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::time::Duration;

fn count(conn: &Connection) -> i64 {
    conn.query_row("SELECT COUNT(*) FROM t", [], |r| r.get(0)).unwrap()
}

#[test]
fn dryspt_ac01() {
    let path = std::env::temp_dir().join(format!(
        "dryspt-ac01-{}-{}.db",
        std::process::id(),
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
    ));
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch("CREATE TABLE t (v TEXT)").unwrap();
    assert!(conn.is_autocommit());

    let probe = Connection::open(&path).unwrap();
    probe.busy_timeout(Duration::ZERO).unwrap();

    // Ok: lock held while the closure runs, row committed afterwards.
    with_tx(&conn, TxBehavior::Immediate, |c| {
        c.execute("INSERT INTO t VALUES ('ok')", [])?;
        let busy = probe.execute_batch("BEGIN IMMEDIATE");
        let err = busy.expect_err("probe must be refused while the helper holds the write lock");
        assert!(
            matches!(err.sqlite_error_code(), Some(rusqlite::ErrorCode::DatabaseBusy)),
            "expected SQLITE_BUSY, got {err:?}"
        );
        Ok::<_, AppError>(())
    })
    .expect("ok path");
    assert!(conn.is_autocommit());
    assert_eq!(count(&probe), 1, "Ok row is committed and visible to another connection");

    // Err: rolled back.
    let r: Result<(), AppError> = with_tx(&conn, TxBehavior::Immediate, |c| {
        c.execute("INSERT INTO t VALUES ('err')", [])?;
        Err(AppError::Internal("boom".into()))
    });
    assert!(r.is_err());
    assert!(conn.is_autocommit());
    assert_eq!(count(&conn), 1, "Err row is absent");

    // Panic: rolled back, process survives, lock released.
    let p = catch_unwind(AssertUnwindSafe(|| {
        let _ = with_tx(&conn, TxBehavior::Immediate, |c| -> Result<(), AppError> {
            c.execute("INSERT INTO t VALUES ('panic')", [])?;
            panic!("closure panic");
        });
    }));
    assert!(p.is_err(), "panic propagates to the caller");
    assert!(conn.is_autocommit(), "connection back in autocommit after the panic");
    assert_eq!(count(&conn), 1, "panic row is absent");
    probe.execute_batch("BEGIN IMMEDIATE; ROLLBACK;").expect("lock released after the panic");

    let _ = std::fs::remove_file(&path);
}
