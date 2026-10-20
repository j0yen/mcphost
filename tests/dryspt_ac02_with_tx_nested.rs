//! PRD-mcphost-dry-run-inside-savepoint-for-transactional-verbs
//! AC2 -- inside an open `SAVEPOINT test_run`, `with_tx` nests a savepoint,
//! never ends the outer transaction, and `ROLLBACK TO test_run` removes
//! even the `Ok` write.

use mcphost::errors::AppError;
use mcphost::txn::{with_tx, TxBehavior};
use rusqlite::Connection;
use std::panic::{catch_unwind, AssertUnwindSafe};

fn rows(conn: &Connection) -> Vec<String> {
    let mut s = conn.prepare("SELECT v FROM t ORDER BY rowid").unwrap();
    s.query_map([], |r| r.get(0)).unwrap().map(|r| r.unwrap()).collect()
}

#[test]
fn dryspt_ac02() {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch("CREATE TABLE t (v TEXT); SAVEPOINT test_run;").unwrap();
    assert!(!conn.is_autocommit());

    with_tx(&conn, TxBehavior::Immediate, |c| {
        c.execute("INSERT INTO t VALUES ('ok')", [])?;
        Ok::<_, AppError>(())
    })
    .expect("Ok closure");
    assert!(!conn.is_autocommit());

    let r: Result<(), AppError> = with_tx(&conn, TxBehavior::Deferred, |c| {
        c.execute("INSERT INTO t VALUES ('err')", [])?;
        Err(AppError::Internal("boom".into()))
    });
    assert!(r.is_err());
    assert!(!conn.is_autocommit());
    assert_eq!(rows(&conn), vec!["ok"], "Err write undone, Ok write kept");

    let p = catch_unwind(AssertUnwindSafe(|| {
        let _ = with_tx(&conn, TxBehavior::Immediate, |c| -> Result<(), AppError> {
            c.execute("INSERT INTO t VALUES ('panic')", [])?;
            panic!("closure panic");
        });
    }));
    assert!(p.is_err());
    assert!(!conn.is_autocommit(), "outer savepoint survives all three");
    assert_eq!(rows(&conn), vec!["ok"], "panic write undone");

    conn.execute_batch("ROLLBACK TO test_run").expect("outer savepoint still open");
    assert!(rows(&conn).is_empty(), "outer rollback removes the Ok write too");
}
