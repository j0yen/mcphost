//! PRD-mcphost-data-retention
//! AC9 (P1) — Given a prune that errors, When `healthz` is read, Then
//! `last_prune_ok` is false.
//!
//! The prune's batched deletes run on their own freshly-opened SQLite
//! connection (`retention::prune_sync`). Chmod-ing the db file read-only
//! doesn't reliably reproduce the fault on every environment this suite
//! runs on (SQLite's unix VFS can fall back to read-only rather than
//! failing `open()`, and in WAL mode a write can still land in the `-wal`
//! side file), so this forces the failure the same way
//! PRD-mcphost-sqlite-busy-timeout-audit's own AC5 does: a second
//! connection holds the write lock past prune's `busy_timeout` (set to a
//! short 50ms via `TestServer::start_with_db_cfg` so this doesn't wait out
//! the real 5s default), so `delete_batches`'s `DELETE` genuinely hits
//! `SQLITE_BUSY`. The blocker releases its lock on a timer (not after
//! `prune_once` returns): `prune_once` also journals the outcome to
//! `prune_log` on the app's own shared connection (same `busy_timeout`,
//! same file) right after the delete fails, so the lock has to be gone by
//! then too or that bookkeeping write would itself time out and the
//! journal row this test asserts on would never land.
//!
//! The blocker used to release on a fixed sleep timed to straddle those
//! two windows; under the full ~650-test suite's CPU contention the
//! blocker thread's own scheduling could be delayed enough to blow past
//! the timing budget (flaky, not `db`-related). It now releases as soon as
//! it *observes* `ROLE_PRUNE`'s `busy_total` counter increment -- the
//! actual signal that `delete_batches` has already hit `SQLITE_BUSY` --
//! instead of guessing a wall-clock margin, so it holds exactly as long as
//! needed regardless of how loaded the box is.
//!
//! The observation was still racy on one side: `std::thread::spawn`
//! returning does not mean the blocker has run `BEGIN IMMEDIATE` yet, so on
//! a loaded runner `prune_once` could finish its whole delete *before* the
//! blocker took the lock, never hit `SQLITE_BUSY`, and the blocker's poll
//! then timed out ("prune's delete never hit SQLITE_BUSY ..."; GitHub run
//! 38878016104). `prune_once` is therefore not called until the blocker has
//! signalled over a channel that it HOLDS the write lock, and the wait for
//! the busy observation is bounded generously (a multiple of the configured
//! `busy_timeout`, never under a minute) so it is a hang guard, not a
//! timing assertion. `busy_timeout` is also 1s rather than 50ms: after the
//! delete fails, the `prune_log` journal write on the shared connection
//! must find the lock already released, and the blocker only releases
//! after its next poll observes the counter -- 50ms left that scheduling
//! gap no margin on a starved runner (the journal write would time out and
//! `last_prune_ok` would stay true), 1s does.

use crate::common;
use common::ADMIN_KEY;

/// prune's (and the shared connection's) `busy_timeout`. The blocker holds
/// the lock past this, so prune's `DELETE` genuinely exhausts it.
const BUSY_TIMEOUT_MS: u64 = 1_000;
/// How often the blocker re-reads `ROLE_PRUNE`'s `busy_total`.
const POLL_MS: u64 = 5;
/// Hang guard for the busy observation and the lock handshake: 6x the
/// configured `busy_timeout`, never under 60s. Iteration-counted, not
/// clocked, so no wall-clock value is ever asserted on.
const OBSERVE_BOUND_MS: u64 = if 6 * BUSY_TIMEOUT_MS > 60_000 { 6 * BUSY_TIMEOUT_MS } else { 60_000 };

async fn admin_healthz(base_url: &str) -> serde_json::Value {
    reqwest::Client::new()
        .get(format!("{base_url}/healthz"))
        .bearer_auth(ADMIN_KEY)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap()
}

#[tokio::test]
async fn prune_that_hits_a_held_write_lock_sets_last_prune_ok_false() {
    let server =
        common::TestServer::start_with_db_cfg(mcphost::db::DbConfig { busy_timeout_ms: BUSY_TIMEOUT_MS }).await;
    let (namespace, _key) = common::signup(&server.base_url, "AC9 Tenant").await;
    let tenant = server
        .state
        .db
        .find_tenant_by_namespace(namespace)
        .await
        .expect("query tenant")
        .expect("tenant exists");
    server
        .state
        .db
        .insert_calls_row_for_test(tenant.id, mcphost::state::now_unix() - 91 * 86_400)
        .await
        .expect("seed a calls row old enough that delete_batches attempts a real write");
    // The seeded row (91 days old) must be inside the `calls` retention
    // window, or prune would have nothing eligible to write.
    let windows = server.state.db.retention_windows().await.expect("read retention windows");
    let calls_days = windows
        .iter()
        .find(|(name, _)| name == "calls")
        .map(|(_, days)| *days)
        .expect("a `calls` retention window exists");
    assert!(
        calls_days < 91,
        "seed precondition: the 91-day-old calls row must be older than the `calls` retention window ({calls_days} days), else prune has nothing to delete"
    );

    let health = admin_healthz(&server.base_url).await;
    assert_eq!(
        health["last_prune_ok"],
        serde_json::json!(true),
        "last_prune_ok must be true before any prune has run"
    );

    let db_path = server.state.db.data_dir().join("mcphost.db");
    let db_for_poll = server.state.db.clone();
    let busy_before = db_for_poll.counters(mcphost::db::ROLE_PRUNE).busy_total;
    let (lock_held_tx, lock_held_rx) = std::sync::mpsc::channel();
    let blocker_handle = std::thread::spawn(move || {
        let blocker = rusqlite::Connection::open(&db_path).expect("open blocker connection");
        // rusqlite's own default busy handler is only 5s; the app's shared
        // connection may be mid-write on a loaded runner, so wait it out.
        blocker
            .busy_timeout(std::time::Duration::from_millis(OBSERVE_BOUND_MS))
            .expect("set blocker busy_timeout");
        blocker
            .execute_batch("BEGIN IMMEDIATE;")
            .expect("blocker acquires the write lock");
        // The handshake: only now may `prune_once` start.
        let _ = lock_held_tx.send(());
        // Release as soon as prune's own busy_total counter proves
        // `delete_batches` has already hit `SQLITE_BUSY`, rather than a
        // fixed sleep -- robust regardless of how loaded the box is.
        for _ in 0..(OBSERVE_BOUND_MS / POLL_MS) {
            if db_for_poll.counters(mcphost::db::ROLE_PRUNE).busy_total > busy_before {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(POLL_MS));
        }
        let observed = db_for_poll.counters(mcphost::db::ROLE_PRUNE).busy_total > busy_before;
        // Always release, even when the observation failed, so a failure
        // here surfaces as an assertion instead of a wedged lock.
        blocker.execute_batch("ROLLBACK;").expect("release the write lock");
        assert!(
            observed,
            "prune's delete never hit SQLITE_BUSY while the blocker held the write lock; busy_total stayed at {busy_before}"
        );
    });

    // Do not start prune until the blocker is confirmed to HOLD the lock;
    // otherwise prune can finish its delete before the blocker has taken it.
    if lock_held_rx.recv().is_err() {
        // The blocker died before signalling; surface its panic.
        blocker_handle.join().expect("blocker thread failed before acquiring the write lock");
        panic!("blocker thread exited without signalling that it holds the write lock");
    }

    let result = server.state.db.prune_once().await;

    blocker_handle.join().expect("blocker thread");

    result.expect_err("prune_once must fail when the write lock is held past busy_timeout");

    let health = admin_healthz(&server.base_url).await;
    assert_eq!(
        health["last_prune_ok"],
        serde_json::json!(false),
        "healthz must report last_prune_ok: false after a prune that errored"
    );
}
