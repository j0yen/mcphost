//! PRD-mcphost-synthetic-tenant-purge
//! AC4 (P0) — Given implicit synthetic unclaimed rows minted before the floor,
//! When the sweep runs with the window elapsed since the floor, Then each is
//! disabled `idle_expired` with a lifecycle event, the expire stage stops at the
//! wall budget reporting `held_over`, a row idle under the window is untouched,
//! and the sweep with no prior `prune_log` row carrying `purge_due_unix` logs
//! `pre_floor` with a <= 200-row `pre_floor_sample`, no key or address.

use crate::common::tenp::{self, DAY, Row};
use crate::common::{EnvGuard, TestServer};
use serde_json::Value;

const PRE_FLOOR_IDLE: usize = 700;
const BATCH: i64 = 500;

fn live(ns: &str, created_unix: i64, last_seen_unix: i64) -> Row {
    Row { disabled: false, reason: None, created_unix, last_seen_unix: Some(last_seen_unix), ..Row::subject(ns, last_seen_unix) }
}

fn lifecycle(conn: &rusqlite::Connection, id: i64) -> Vec<(String, String, Option<i64>, i64)> {
    let mut stmt = conn
        .prepare("SELECT policy, stage, idle_since_unix, created_unix FROM tenant_lifecycle_events WHERE tenant_id = ?1")
        .unwrap();
    stmt.query_map([id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))).unwrap().map(|r| r.unwrap()).collect()
}

fn state(conn: &rusqlite::Connection, id: i64) -> (bool, Option<String>) {
    crate::common::tenx::state(conn, id)
}

#[tokio::test]
async fn tenp_ac04_pre_floor_rows_expire_from_the_floor_and_the_first_sweep_lists_them() {
    // A budget one sweep cannot finish: each sweep's expire stage stops after one batch.
    let _env = EnvGuard::set(tenp::BUDGET_ENV, "0").await;
    let server = TestServer::start().await;
    server.state.db.seed_retention_policy_from_env().await.expect("seed");
    let conn = tenp::open_raw(&server.data_dir.0);
    let now = mcphost::state::now_unix();
    let floor = now - 10 * DAY;
    crate::common::tenx::set_floor(&conn, floor);
    tenp::seed_pre_prd_row(&conn, now);

    // Minted 20 days ago, last seen 15 days ago: before the floor, idle since the floor.
    conn.execute_batch("BEGIN").unwrap();
    let pre_floor: Vec<i64> = (0..PRE_FLOOR_IDLE)
        .map(|i| tenp::insert(&conn, &live(&format!("tenp-pre-{i}"), now - 20 * DAY, now - 15 * DAY)))
        .collect();
    // An address on file never reaches the log: `owner_email` set, still unverified.
    conn.execute("UPDATE tenants SET owner_email = 'someone@example.com' WHERE id = ?1", [pre_floor[0]]).unwrap();
    // Minted before the floor but seen an hour ago: idle under the window.
    let pre_floor_active = tenp::insert(&conn, &live("tenp-pre-active", now - 20 * DAY, now - 3600));
    // After the floor, idle two days: a subject under either rule.
    let post_floor = tenp::insert(&conn, &live("tenp-post", now - 3 * DAY, now - 2 * DAY));
    // Never subjects.
    let claimed = tenp::insert(&conn, &Row { claimed_unix: Some(now - DAY), ..live("tenp-claimed", now - 20 * DAY, now - 15 * DAY) });
    let explicit = tenp::insert(&conn, &Row { signup_source: Some("explicit"), ..live("tenp-explicit", now - 20 * DAY, now - 15 * DAY) });
    let external = tenp::insert(
        &conn,
        &Row { origin: "external", synthetic: None, ..live("tenp-external", now - 20 * DAY, now - 15 * DAY) },
    );
    conn.execute_batch("COMMIT").unwrap();
    let subjects = (PRE_FLOOR_IDLE + 1) as i64;

    // Sweep 1: one batch of 500, the rest held over; the gate is closed, so the backlog is listed.
    let first = server.state.db.prune_once().await.expect("sweep 1").synthetic;
    assert_eq!(first["expired"], BATCH, "{first}");
    assert_eq!(first["expire_held_over"], subjects - BATCH, "{first}");
    let flat: Value = serde_json::from_str(
        &conn.query_row::<String, _, _>("SELECT deleted_json FROM prune_log ORDER BY id DESC LIMIT 1", [], |r| r.get(0)).unwrap(),
    )
    .unwrap();
    assert_eq!(flat["synthetic_implicit_tenants.held_over"], subjects - BATCH, "the expire stage reports held_over: {flat}");
    assert_eq!(flat["synthetic_implicit_tenants.expired"], BATCH);
    // pre_floor counts every enabled class row minted before the floor, as the sweep found them.
    assert_eq!(first["pre_floor"], (PRE_FLOOR_IDLE + 1) as i64, "{first}");
    let sample = first["pre_floor_sample"].as_array().expect("the gate was closed: a sample is logged");
    assert_eq!(sample.len(), 200, "at most 200 rows");
    for row in sample {
        let keys: std::collections::BTreeSet<&str> = row.as_object().unwrap().keys().map(String::as_str).collect();
        assert_eq!(keys, ["created_unix", "display_name", "idle_days", "tenant_id"].into_iter().collect(), "{row}");
    }
    assert_eq!(sample[0]["idle_days"], 10, "the idle run starts at the floor, 10 days back");
    let logged = first.to_string();
    for secret in ["someone@example.com", "hash-tenp-pre-0", "key_hash", "owner_email", "claim_token"] {
        assert!(!logged.contains(secret), "the log carries no key or address: {secret}");
    }

    let expired_after_first: Vec<i64> = pre_floor
        .iter()
        .chain([&post_floor])
        .copied()
        .filter(|id| state(&conn, *id) == (true, Some("idle_expired".to_string())))
        .collect();
    assert_eq!(expired_after_first.len() as i64, BATCH);
    for id in &expired_after_first {
        let events = lifecycle(&conn, *id);
        assert_eq!(events.len(), 1, "one lifecycle event per disable");
        let (policy, stage, idle_since, created) = &events[0];
        assert_eq!((policy.as_str(), stage.as_str()), ("synthetic_implicit_tenants", "invalidated"));
        assert!(*created >= now, "the event is dated by the sweep");
        let expected_idle_since = if *id == post_floor { now - 2 * DAY } else { floor };
        assert_eq!(*idle_since, Some(expected_idle_since), "the idle run began at the floor for a pre-floor row");
    }

    // Sweep 2 finishes the remainder; the gate is armed now, so no sample, but the count stays.
    let second = server.state.db.prune_once().await.expect("sweep 2").synthetic;
    assert_eq!(second["expired"], subjects - BATCH, "{second}");
    assert_eq!(second["expire_held_over"], 0, "{second}");
    assert!(second.get("pre_floor_sample").is_none(), "only the sweep that finds no armed row lists the backlog: {second}");
    assert_eq!(second["pre_floor"], (PRE_FLOOR_IDLE + 1) as i64 - BATCH, "the 200 not yet disabled and the active row: {second}");
    for id in pre_floor.iter().chain([&post_floor]) {
        assert_eq!(state(&conn, *id), (true, Some("idle_expired".to_string())), "subject {id}");
        assert_eq!(lifecycle(&conn, *id).len(), 1, "never a second event");
    }
    for (label, id) in [("pre-floor row seen an hour ago", pre_floor_active), ("claimed", claimed), ("explicit", explicit), ("external", external)] {
        assert_eq!(state(&conn, id), (false, None), "{label} is untouched");
        assert!(lifecycle(&conn, id).is_empty(), "{label} gets no event");
    }

    // The clock starts at the floor: with the floor an hour back, a row last seen long
    // before it is not idle yet, however old it is.
    crate::common::tenx::set_floor(&conn, now - 3600);
    let old = tenp::insert(&conn, &live("tenp-old-row", now - 30 * DAY, now - 30 * DAY));
    let third = server.state.db.prune_once().await.expect("sweep 3").synthetic;
    assert_eq!(third["expired"], 0, "{third}");
    assert_eq!(state(&conn, old), (false, None), "the window has not elapsed since the floor");
    crate::common::tenx::set_floor(&conn, now - 2 * DAY);
    let fourth = server.state.db.prune_once().await.expect("sweep 4").synthetic;
    assert_eq!(fourth["expired"], 1, "the old row, now two days idle since the floor: {fourth}");
    assert_eq!(state(&conn, old), (true, Some("idle_expired".to_string())));
    assert_eq!(state(&conn, pre_floor_active), (false, None), "the row seen an hour ago stays");
}
