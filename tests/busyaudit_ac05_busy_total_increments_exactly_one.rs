//! PRD-mcphost-sqlite-busy-timeout-audit
//! AC5 — Given a write lock held longer than the busy timeout, When a
//! statement runs, Then `db_busy_total` increments by exactly one and the
//! caller receives the existing structured error.
//!
//! Uses a short `busy_timeout` (50ms, via `TestServer::start_with_db_cfg`
//! -- see that helper's own doc comment on why this isn't a process env
//! var) so the test doesn't wait out the real 5s default, and holds the
//! write lock well past it. The "existing structured error" is
//! `AppError::Storage` (see `errors.rs`'s `From<rusqlite::Error>`) --
//! unchanged by this PRD; `note_rusqlite_error` only counts the error,
//! never reshapes it.

use crate::common;

#[tokio::test]
async fn statement_blocked_past_busy_timeout_counts_busy_total_once_and_errors() {
    let server =
        common::TestServer::start_with_db_cfg(mcphost::db::DbConfig { busy_timeout_ms: 50 }).await;
    let db_path = server.state.db.data_dir().join("mcphost.db");

    let before = server.state.db.counters(mcphost::db::ROLE_SERVER);

    let (lock_acquired_tx, lock_acquired_rx) = std::sync::mpsc::channel();
    let blocker_handle = std::thread::spawn(move || {
        let blocker = rusqlite::Connection::open(&db_path).expect("open blocker connection");
        blocker
            .execute_batch("BEGIN IMMEDIATE;")
            .expect("blocker acquires the write lock");
        let _ = lock_acquired_tx.send(());
        // Well past the 50ms busy_timeout, so the statement below is
        // guaranteed to exhaust its retry budget and fail.
        std::thread::sleep(std::time::Duration::from_millis(300));
        blocker.execute_batch("ROLLBACK;").expect("release the write lock");
    });

    lock_acquired_rx.recv().expect("blocker signals lock acquired");

    let result = server
        .state
        .db
        .record_admin_audit(
            "test-actor".to_string(),
            "busyaudit_ac05".to_string(),
            None,
            None,
        )
        .await;

    blocker_handle.join().expect("blocker thread");

    let err = result.expect_err("the statement must fail once busy_timeout is exhausted");
    assert_eq!(
        err.code(),
        "storage",
        "the caller must see the existing structured storage error, not a new one: {err:?}"
    );

    let after = server.state.db.counters(mcphost::db::ROLE_SERVER);
    assert_eq!(
        after.busy_total,
        before.busy_total + 1,
        "db_busy_total must increment by exactly one: before={before:?} after={after:?}"
    );
    assert_eq!(
        after.locked_total, before.locked_total,
        "this scenario is SQLITE_BUSY, not SQLITE_LOCKED: before={before:?} after={after:?}"
    );
}
