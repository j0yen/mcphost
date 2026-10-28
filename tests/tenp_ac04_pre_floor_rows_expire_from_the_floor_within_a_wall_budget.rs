//! PRD-mcphost-synthetic-tenant-purge
//! AC4 (P0) — Given implicit synthetic unclaimed rows minted before the floor, When the sweep runs
//! with the window elapsed since the floor, Then each is disabled idle_expired with a lifecycle
//! event, the expire stage stops at the wall budget reporting held_over, a row idle under the window
//! is untouched, and the sweep with no prior prune_log row carrying purge_due_unix logs pre_floor
//! with a ≤ 200-row pre_floor_sample, no key or address.

use crate::common::tenp::{self, DAY, Row};
use crate::common::{EnvGuard, TestServer};
use rusqlite::{Connection, params};
use serde_json::Value;

const ROWS: usize = 1_200;

fn events(conn: &Connection, id: i64) -> Vec<(String, String, Option<i64>)> {
    conn.prepare("SELECT policy, stage, idle_since_unix FROM tenant_lifecycle_events WHERE tenant_id = ?1 ORDER BY id")
        .unwrap()
        .query_map(params![id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap()
}

/// An implicit synthetic unclaimed row minted `age_days` ago, never seen.
fn unseen(ns: &str, now: i64, age_days: i64) -> Row {
    Row { last_seen_unix: None, ..Row::implicit(ns, now, age_days, 0) }
}

#[tokio::test]
async fn tenp_ac04_pre_floor_rows_are_disabled_in_budgeted_batches_and_the_first_sweep_lists_them() {
    // A budget no stage can use up: the expire stage runs one batch of EXPIRE_BATCH_SIZE and stops.
    let spent = EnvGuard::set(tenp::BUDGET_ENV, "0").await;
    let server = TestServer::start().await;
    server.state.db.seed_retention_policy_from_env().await.expect("seed");
    let mut conn = Connection::open(server.data_dir.0.join("mcphost.db")).expect("raw db");
    let now = mcphost::state::now_unix();
    let floor = now - 10 * DAY;
    tenp::set_floor(&conn, floor);

    let tx = conn.transaction().unwrap();
    let mut subjects = Vec::new();
    for i in 0..ROWS {
        // Minted 20 days ago, i.e. before the floor; never seen, so the idle run starts at the floor.
        subjects.push(tenp::insert(&tx, &unseen(&format!("tenp4-pre-{i}"), now, 20)));
    }
    // Pre-floor but seen an hour ago: idle under the window, untouched.
    let recent = tenp::insert(&tx, &Row { last_seen_unix: Some(now - 3600), ..unseen("tenp4-recent", now, 20) });
    // Minted after the floor and idle past the window: a subject as before.
    let post_floor = tenp::insert(&tx, &Row { last_seen_unix: Some(now - 2 * DAY), ..unseen("tenp4-post", now, 5) });
    tx.commit().unwrap();

    // Sweep 1: no earlier row carries `purge_due_unix`, so it lists the backlog; the budget stops it
    // after one batch.
    server.state.db.prune_once().await.expect("first sweep");
    let first = tenp::last_block(&conn);
    let log: Value = serde_json::from_str(
        &conn.query_row("SELECT deleted_json FROM prune_log ORDER BY id DESC LIMIT 1", [], |r| r.get::<_, String>(0)).unwrap(),
    )
    .unwrap();
    let expired = log["synthetic_implicit_tenants.expired"].as_i64().unwrap();
    assert_eq!(expired, 500, "one EXPIRE_BATCH_SIZE batch, then the budget is spent: {log}");
    let held_over = log["synthetic_implicit_tenants.held_over"].as_i64().unwrap();
    assert_eq!(held_over, (ROWS + 1) as i64 - 500, "the rest of the idle subjects wait, `recent` is not one: {log}");
    assert_eq!(first["expire_held_over"], held_over, "{first}");
    assert_eq!(first["expired"], 500, "{first}");
    assert_eq!(first["pre_floor"], (ROWS + 1) as i64, "the backlog counted at the start of the sweep: {first}");
    let sample = first["pre_floor_sample"].as_array().expect("the first sweep lists the backlog");
    assert_eq!(sample.len(), 200, "at most 200 rows: {first}");
    for row in sample {
        let keys: Vec<&str> = row.as_object().unwrap().keys().map(String::as_str).collect();
        assert_eq!(keys, ["created_unix", "display_name", "idle_days", "tenant_id"], "no key, secret or address: {row}");
        assert_eq!(row["created_unix"], now - 20 * DAY);
    }
    assert!(!first.to_string().contains("hash-"), "no key hash in the block");
    assert!(first.as_object().unwrap().contains_key("purge_due_unix"), "the gate token is written: {first}");

    let disabled: Vec<i64> = subjects.iter().copied().filter(|id| tenp::state(&conn, *id).0).collect();
    assert_eq!(disabled.len(), 500, "exactly the batch is disabled");
    for id in &disabled {
        assert_eq!(tenp::state(&conn, *id), (true, Some("idle_expired".to_string())));
        assert_eq!(
            events(&conn, *id),
            vec![("synthetic_implicit_tenants".to_string(), "invalidated".to_string(), Some(floor))],
            "one lifecycle event; the idle run began at the floor"
        );
    }
    assert_eq!(tenp::state(&conn, recent), (false, None), "idle under the window: untouched");
    assert!(events(&conn, recent).is_empty());

    // Sweep 2 (a spent budget again) takes the next batch and no longer lists the backlog.
    server.state.db.prune_once().await.expect("second sweep");
    let second = tenp::last_block(&conn);
    assert_eq!(second["expired"], 500, "{second}");
    assert!(second.get("pre_floor_sample").is_none(), "the list is written once: {second}");
    assert_eq!(second["pre_floor"], (ROWS + 1) as i64 - 500, "the backlog count is on every sweep: {second}");
    drop(spent);

    // With a real budget the rest goes in one sweep, including the post-floor row.
    let _real = EnvGuard::clear(tenp::BUDGET_ENV).await;
    server.state.db.prune_once().await.expect("third sweep");
    let third = tenp::last_block(&conn);
    assert_eq!(third["expire_held_over"], 0, "{third}");
    let all_disabled = subjects.iter().filter(|id| tenp::state(&conn, **id).0).count();
    assert_eq!(all_disabled, ROWS, "every pre-floor idle subject is disabled over the sweeps");
    assert_eq!(tenp::state(&conn, post_floor), (true, Some("idle_expired".to_string())));
    assert_eq!(tenp::state(&conn, recent), (false, None), "still untouched");
    let total: i64 = conn.query_row("SELECT COUNT(*) FROM tenant_lifecycle_events WHERE stage = 'invalidated'", [], |r| r.get(0)).unwrap();
    assert_eq!(total, ROWS as i64 + 1, "one event per disabled row");
}

#[tokio::test]
async fn tenp_ac04_the_idle_clock_of_a_pre_floor_row_starts_at_the_floor() {
    let _env = EnvGuard::clear(tenp::BUDGET_ENV).await;
    let server = TestServer::start().await;
    server.state.db.seed_retention_policy_from_env().await.expect("seed");
    let conn = Connection::open(server.data_dir.0.join("mcphost.db")).expect("raw db");
    let now = mcphost::state::now_unix();
    // Minted 30 days ago, last seen 29 days ago; the policy shipped 12 hours ago.
    tenp::set_floor(&conn, now - DAY / 2);
    let id = tenp::insert(&conn, &Row { last_seen_unix: Some(now - 29 * DAY), ..unseen("tenp4-clock", now, 30) });
    server.state.db.prune_once().await.expect("sweep before the window since the floor");
    assert_eq!(tenp::state(&conn, id), (false, None), "12 hours since the floor is under the one-day window");
    assert_eq!(tenp::last_block(&conn)["pre_floor"], 1);

    tenp::set_floor(&conn, now - DAY - 3600);
    server.state.db.prune_once().await.expect("sweep after the window since the floor");
    assert_eq!(tenp::state(&conn, id), (true, Some("idle_expired".to_string())), "the window has elapsed since the floor");
    assert_eq!(events(&conn, id)[0].2, Some(now - DAY - 3600), "the event records the idle run's start: the floor");
}
