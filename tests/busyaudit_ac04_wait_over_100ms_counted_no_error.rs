//! PRD-mcphost-sqlite-busy-timeout-audit
//! AC4 — Given a second process holds a write lock for 200 ms, When a
//! statement runs, Then `db_wait_gt100ms_total` increments,
//! `db_wait_max_ms ≥ 200`, and no error is returned.
//!
//! `admin.db.stats` (AC7) doesn't exist yet at this AC -- this reads the
//! same counters straight off `Db::counters`, which that RPC will later
//! wrap. "A second process" is simulated as a second real
//! `rusqlite::Connection` to the same file on its own OS thread: SQLite's
//! locking is connection-scoped, not process-scoped, so this contends
//! exactly the way a second process would.

use crate::common;

#[tokio::test]
async fn statement_blocked_over_100ms_counts_the_wait_and_returns_no_error() {
    let server = common::TestServer::start().await;
    let db_path = server.state.db.data_dir().join("mcphost.db");

    let before = server.state.db.counters(mcphost::db::ROLE_SERVER);

    let (lock_acquired_tx, lock_acquired_rx) = std::sync::mpsc::channel();
    let blocker_handle = std::thread::spawn(move || {
        let blocker = rusqlite::Connection::open(&db_path).expect("open blocker connection");
        blocker
            .execute_batch("BEGIN IMMEDIATE;")
            .expect("blocker acquires the write lock");
        let _ = lock_acquired_tx.send(());
        // A margin above the 200ms floor: the writer's measured wait
        // starts once it actually attempts the write (a few ms after this
        // signal, on the tokio side), so holding exactly 200ms would
        // measure a few ms under it.
        std::thread::sleep(std::time::Duration::from_millis(220));
        blocker.execute_batch("ROLLBACK;").expect("release the write lock");
    });

    // Wait for the blocker to actually hold the lock before attempting
    // the write below, so its whole wait counts against the hold.
    lock_acquired_rx.recv().expect("blocker signals lock acquired");

    let result = server
        .state
        .db
        .record_admin_audit(
            "test-actor".to_string(),
            "busyaudit_ac04".to_string(),
            None,
            None,
        )
        .await;

    blocker_handle.join().expect("blocker thread");

    result.expect("the statement must eventually succeed once the lock is released");

    let after = server.state.db.counters(mcphost::db::ROLE_SERVER);
    assert!(
        after.wait_gt100ms_total > before.wait_gt100ms_total,
        "wait_gt100ms_total must increment: before={before:?} after={after:?}"
    );
    assert!(
        after.wait_max_ms >= 200,
        "wait_max_ms must reflect the ~200ms wait: before={before:?} after={after:?}"
    );
}
