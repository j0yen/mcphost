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

use crate::common;
use common::ADMIN_KEY;

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
        common::TestServer::start_with_db_cfg(mcphost::db::DbConfig { busy_timeout_ms: 50 }).await;
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

    let health = admin_healthz(&server.base_url).await;
    assert_eq!(
        health["last_prune_ok"],
        serde_json::json!(true),
        "last_prune_ok must be true before any prune has run"
    );

    let db_path = server.state.db.data_dir().join("mcphost.db");
    let db_for_poll = server.state.db.clone();
    let busy_before = db_for_poll.counters(mcphost::db::ROLE_PRUNE).busy_total;
    let blocker_handle = std::thread::spawn(move || {
        let blocker = rusqlite::Connection::open(&db_path).expect("open blocker connection");
        blocker
            .execute_batch("BEGIN IMMEDIATE;")
            .expect("blocker acquires the write lock");
        // Release as soon as prune's own busy_total counter proves
        // `delete_batches` has already hit `SQLITE_BUSY`, rather than a
        // fixed sleep -- robust regardless of how loaded the box is.
        for _ in 0..2000 {
            if db_for_poll.counters(mcphost::db::ROLE_PRUNE).busy_total > busy_before {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(
            db_for_poll.counters(mcphost::db::ROLE_PRUNE).busy_total > busy_before,
            "prune's delete never hit SQLITE_BUSY within 10s; blocker never observed it"
        );
        blocker.execute_batch("ROLLBACK;").expect("release the write lock");
    });

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
