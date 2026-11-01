//! PRD-mcphost-synthetic-implicit-tenant-expiry
//! AC2 (P0) — Given the env var unset, set to `3`, and set to `0`, When the
//! policy table is seeded at start twice and `prune_once` runs on fixture A
//! (idle 2 days), Then the stored window is 1, 3 and 0 days, `floor_unix` is
//! set by the first seed and unchanged by the second, A is expired only in the
//! unset case, and `retention_policy` lists `synthetic_implicit_tenants` beside
//! the existing rows.

use crate::common::tenx::{self, DAY, Fx};
use crate::common::{EnvGuard, TestServer};

/// Seeds twice, runs one sweep over fixture A, returns `(stored days, expired)`.
async fn run(server: &TestServer) -> (i64, bool) {
    let db = &server.state.db;
    db.seed_retention_policy_from_env().await.expect("first seed");
    let conn = rusqlite::Connection::open(server.data_dir.0.join("mcphost.db")).expect("raw db");
    let first = tenx::floor(&conn).expect("first seed sets floor_unix");
    // Push the floor into the past so a second seed that overwrote it would show.
    let now = mcphost::state::now_unix();
    let moved = now - 10 * DAY;
    assert!(moved < first);
    tenx::set_floor(&conn, moved);
    db.seed_retention_policy_from_env().await.expect("second seed");
    assert_eq!(tenx::floor(&conn), Some(moved), "the second seed leaves floor_unix alone");

    let a = tenx::insert(&conn, &Fx::subject("tenx-a", now, 3, 2 * DAY));
    db.prune_once().await.expect("prune_once");
    let (disabled, reason) = tenx::state(&conn, a);
    assert_eq!(reason.is_some(), disabled);
    (tenx::stored_days(&conn), disabled)
}

#[tokio::test]
async fn tenx_ac02_env_sets_the_window_and_zero_switches_it_off() {
    {
        let _env = EnvGuard::clear(tenx::ENV).await;
        let server = TestServer::start().await;
        assert_eq!(run(&server).await, (1, true), "unset: default 1 day, A (idle 2 days) expires");
        let names: Vec<String> =
            server.state.db.retention_windows().await.expect("windows").into_iter().map(|(n, _)| n).collect();
        assert!(names.contains(&tenx::POLICY.to_string()), "listed: {names:?}");
        for existing in ["calls", "events", "runs", "blobs"] {
            assert!(names.contains(&existing.to_string()), "existing row {existing} still listed");
        }
    }
    {
        let _env = EnvGuard::set(tenx::ENV, "3").await;
        let server = TestServer::start().await;
        assert_eq!(run(&server).await, (3, false), "3 days: A idle 2 days stays");
    }
    {
        let _env = EnvGuard::set(tenx::ENV, "0").await;
        let server = TestServer::start().await;
        assert_eq!(run(&server).await, (0, false), "0: stored as 0, the sweep is off");
    }
}
