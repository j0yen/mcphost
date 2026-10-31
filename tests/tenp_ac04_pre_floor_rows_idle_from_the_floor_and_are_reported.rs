//! PRD-mcphost-synthetic-tenant-purge
//! AC4 (P0) — Given implicit synthetic unclaimed rows minted before the floor,
//! When the sweep runs with the window elapsed since the floor, Then each is
//! disabled idle_expired with a lifecycle event, the expire stage stops at the
//! wall budget reporting held_over, a row idle under the window is untouched,
//! and the sweep with no prior prune_log row carrying purge_due_unix logs
//! pre_floor with a ≤ 200-row pre_floor_sample, no key or address.

use crate::common::tenp::{self, BUDGET_ENV, DAY, POLICY};
use crate::common::tenx::{self, Fx};
use crate::common::{EnvGuard, TestServer};
use serde_json::Value;

/// Pre-floor rows the idle clock takes once the window has elapsed since the floor.
const DUE: i64 = 620;

fn disabled_idle_expired(conn: &rusqlite::Connection) -> i64 {
    tenp::count(conn, "SELECT COUNT(*) FROM tenants WHERE disabled_reason = 'idle_expired'")
}

#[tokio::test]
async fn tenp_ac04_pre_floor_rows_idle_from_the_floor_and_the_first_sweep_names_them() {
    // A zero budget: the expire stage stops after one batch of EXPIRE_BATCH_SIZE.
    let _budget = EnvGuard::set(BUDGET_ENV, "0").await;
    let server = TestServer::start().await;
    let db = &server.state.db;
    db.seed_retention_policy_from_env().await.expect("seed");
    let mut conn = rusqlite::Connection::open(server.data_dir.0.join("mcphost.db")).expect("raw db");
    let now = mcphost::state::now_unix();
    let window = tenx::stored_days(&conn) * DAY;
    assert_eq!(window, DAY, "the default window is one day");

    // The floor was set one hour ago: every row below was minted before it.
    let floor_recent = now - 3600;
    tenx::set_floor(&conn, floor_recent);
    let tx = conn.transaction().expect("tx");
    for i in 0..DUE {
        // Created and last seen long before the floor.
        let mut fx = Fx::subject(&format!("tenp-pre-{i:04}"), now, 60, 40 * DAY);
        fx.created_unix = now - 60 * DAY;
        tenx::insert(&tx, &fx);
    }
    // Pre-floor, but seen an hour ago: idle under the window whatever the floor.
    tenx::insert(&tx, &Fx::subject("tenp-pre-seen", now, 60, 3600));
    // Pre-floor rows of other classes are not subjects and are not counted.
    let mut claimed = Fx::subject("tenp-pre-claimed", now, 60, 40 * DAY);
    claimed.claimed_unix = Some(now - 50 * DAY);
    tenx::insert(&tx, &claimed);
    let mut explicit = Fx::subject("tenp-pre-explicit", now, 60, 40 * DAY);
    explicit.signup_source = Some("explicit");
    tenx::insert(&tx, &explicit);
    // An address and a key hash that must never reach the log.
    tx.execute("UPDATE tenants SET owner_email = 'leak@example.com' WHERE namespace LIKE 'tenp-pre-%'", [])
        .expect("addresses");
    tx.commit().expect("commit");
    let class_rows = DUE + 1;

    // Sweep 1: the window has not elapsed since the floor (one hour ago), so nothing is idle.
    let first = db.prune_once().await.expect("first sweep");
    let block = &first.synthetic;
    assert_eq!(disabled_idle_expired(&conn), 0, "idle is measured from the floor, not from the old last_seen: {block}");
    assert_eq!(block["expired"], 0, "{block}");
    assert_eq!(block["pre_floor"], class_rows, "{block}");
    assert_eq!(block["would_expire"], class_rows, "every class row is not yet idle: {block}");
    let sample = block["pre_floor_sample"].as_array().expect("the first sweep lists the pre-floor rows");
    assert_eq!(sample.len(), mcphost::retention::PRE_FLOOR_SAMPLE_LIMIT, "capped at 200 of {class_rows}");
    for row in sample {
        let keys: Vec<&str> = row.as_object().unwrap().keys().map(String::as_str).collect();
        assert_eq!(keys, ["created_unix", "display_name", "idle_days", "tenant_id"], "{row}");
        assert_eq!(row["idle_days"], 0, "idle since the floor, one hour ago: {row}");
    }
    let logged = tenp::last_block(&conn).to_string();
    assert!(!logged.contains("example.com") && !logged.contains("hash-"), "no address or key in the log: {logged}");

    // The window now elapsed since the floor: 30 days ago.
    tenx::set_floor(&conn, now - 30 * DAY);
    let second = db.prune_once().await.expect("second sweep");
    let block = &second.synthetic;
    let batch = mcphost::retention::EXPIRE_BATCH_SIZE;
    assert_eq!(block["expired"], batch, "one batch, then the wall budget: {block}");
    assert_eq!(block["expire_held_over"], DUE - batch, "the expire stage reports its remainder: {block}");
    assert_eq!(block["held_over"], DUE - batch, "the remainder is reported: {block}");
    assert_eq!(block["pre_floor"], class_rows, "{block}");
    assert!(block.get("pre_floor_sample").is_none(), "an armed sweep does not list the rows again: {block}");
    assert_eq!(disabled_idle_expired(&conn), batch);

    // Each disabled row carries one lifecycle event dated from the floor.
    let events: Vec<(String, String, i64)> = conn
        .prepare(
            "SELECT e.policy, e.stage, e.idle_since_unix FROM tenant_lifecycle_events e \
             JOIN tenants t ON t.id = e.tenant_id WHERE t.disabled_reason = 'idle_expired'",
        )
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(events.len() as i64, batch);
    let floor = tenx::floor(&conn).expect("floor");
    assert!(events.iter().all(|(p, s, since)| p == POLICY && s == "invalidated" && *since == floor), "{:?}", &events[..3]);

    // The next sweep takes the rest; the row seen an hour ago and the other classes are untouched.
    let third = db.prune_once().await.expect("third sweep");
    let block: &Value = &third.synthetic;
    assert_eq!(block["expired"], DUE - batch, "{block}");
    assert_eq!((block["expire_held_over"].as_i64(), block["held_over"].as_i64()), (Some(0), Some(0)), "{block}");
    assert_eq!(block["would_expire"], 1, "only the row seen an hour ago is not yet idle: {block}");
    assert_eq!(disabled_idle_expired(&conn), DUE);
    assert_eq!(tenp::count(&conn, "SELECT COUNT(*) FROM tenant_lifecycle_events"), DUE);
    for ns in ["tenp-pre-seen", "tenp-pre-claimed", "tenp-pre-explicit"] {
        let (disabled, reason) = tenx::state(&conn, conn.query_row("SELECT id FROM tenants WHERE namespace = ?1", [ns], |r| r.get(0)).unwrap());
        assert_eq!((disabled, reason), (false, None), "{ns} is untouched");
    }
}
