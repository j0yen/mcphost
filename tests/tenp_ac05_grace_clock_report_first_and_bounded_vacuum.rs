//! PRD-mcphost-synthetic-tenant-purge
//! AC5 (P0) — Given two-marker rows disabled `idle_expired` or `self_offboard`,
//! When sweeps run on a mock clock, Then none is deleted before
//! MAX(last_seen or created, last lifecycle event, floor) + grace, a sweep with no
//! prior `prune_log` row carrying `purge_due_unix` only reports `would_purge` and
//! `cross_refs`, later sweeps delete via `delete_tenant_as` logging
//! `tenant_purge_synthetic`; each batch ends in `incremental_vacuum(PURGE_VACUUM_PAGES)`,
//! no `VACUUM`.

use crate::common::TestServer;
use crate::common::tenp::{self, DAY, Row};
use rusqlite::{Connection, params};
use serde_json::Value;

fn disabled(ns: &str, reason: &'static str, created: i64, last_seen: Option<i64>) -> Row {
    Row { reason: Some(reason), created_unix: created, last_seen_unix: last_seen, ..Row::subject(ns, 0) }
}

fn event(conn: &Connection, tenant: i64, at: i64) {
    conn.execute(
        "INSERT INTO tenant_lifecycle_events (tenant_id, policy, stage, idle_since_unix, created_unix) \
         VALUES (?1, 'synthetic_implicit_tenants', 'invalidated', ?2, ?2)",
        params![tenant, at],
    )
    .unwrap();
}

async fn sweep(server: &TestServer, at: i64) -> Value {
    server.state.db.prune_once_at(at).await.expect("sweep").synthetic
}

fn purge_audit(conn: &Connection) -> Vec<(String, Value)> {
    let mut stmt = conn
        .prepare("SELECT tenant, detail FROM admin_events WHERE action = 'tenant_purge_synthetic' ORDER BY id")
        .unwrap();
    stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, serde_json::from_str::<Value>(&r.get::<_, String>(1)?).unwrap())))
        .unwrap()
        .map(|r| r.unwrap())
        .collect()
}

#[tokio::test]
async fn tenp_ac05_the_first_sweep_that_carries_the_block_only_reports_then_later_sweeps_delete() {
    let server = TestServer::start().await;
    server.state.db.seed_retention_policy_from_env().await.expect("seed");
    let conn = tenp::open_raw(&server.data_dir.0);
    let floor = mcphost::state::now_unix() - 100 * DAY;
    crate::common::tenx::set_floor(&conn, floor);
    tenp::seed_pre_prd_row(&conn, floor);

    // Due at floor+14d, +15d, +17d and +34d.
    let a = tenp::insert(&conn, &disabled("tenp-a", "idle_expired", floor - 6 * DAY, Some(floor - 5 * DAY)));
    let d = tenp::insert(&conn, &disabled("tenp-d", "idle_expired", floor + DAY, None));
    let b = tenp::insert(&conn, &disabled("tenp-b", "self_offboard", floor, Some(floor + 3 * DAY)));
    let c = tenp::insert(&conn, &disabled("tenp-c", "idle_expired", floor - 2 * DAY, Some(floor - DAY)));
    event(&conn, c, floor + 20 * DAY);

    // A pre-PRD row names the policy but carries no purge_due_unix: the gate is closed.
    let first = sweep(&server, floor + 16 * DAY).await;
    assert_eq!(first["purged"], 0, "{first}");
    assert_eq!(first["would_purge"], 2, "A and D are due and are only counted: {first}");
    assert_eq!(first["purge_due_unix"], floor + 14 * DAY, "the earliest due stamp: {first}");
    assert!(first["cross_refs"].as_object().unwrap().is_empty() && first["held_entangled"].as_object().unwrap().is_empty());
    for id in [a, b, c, d] {
        assert!(tenp::tenant_exists(&conn, id), "report-only deletes nothing");
    }
    assert!(purge_audit(&conn).is_empty());
    let logged: String = conn
        .query_row("SELECT deleted_json FROM prune_log ORDER BY id DESC LIMIT 1", [], |r| r.get(0))
        .unwrap();
    assert!(logged.contains("\"purge_due_unix\""), "the armed token is written: {logged}");

    // The same instant, armed: the due rows go through delete_tenant_as and are audited.
    let second = sweep(&server, floor + 16 * DAY).await;
    assert_eq!(second["purged"], 2, "{second}");
    assert_eq!(second["held_over"], 0);
    assert_eq!(second["purge_due_unix"], floor + 17 * DAY, "the next row to come due: {second}");
    assert!(!tenp::tenant_exists(&conn, a) && !tenp::tenant_exists(&conn, d));
    assert!(tenp::tenant_exists(&conn, b) && tenp::tenant_exists(&conn, c));
    let audit = purge_audit(&conn);
    let names: Vec<&str> = audit.iter().map(|(ns, _)| ns.as_str()).collect();
    assert_eq!(names, vec!["tenp-a", "tenp-d"]);
    for (ns, detail) in &audit {
        assert_eq!(detail["namespace"], ns.as_str(), "{detail}");
        assert!(detail["cross_refs"].is_object(), "{detail}");
    }
    assert_eq!(tenp::count(&conn, "SELECT COUNT(*) FROM admin_events WHERE action = 'tenant_delete'"), 0);
}

#[tokio::test]
async fn tenp_ac05_none_is_deleted_before_the_latest_of_last_seen_event_and_floor_plus_grace() {
    let server = TestServer::start().await;
    server.state.db.seed_retention_policy_from_env().await.expect("seed");
    let conn = tenp::open_raw(&server.data_dir.0);
    let floor = mcphost::state::now_unix() - 100 * DAY;
    crate::common::tenx::set_floor(&conn, floor);
    tenp::arm_gate(&conn, floor);

    // (row, due offset in days from the floor)
    let a = tenp::insert(&conn, &disabled("tenp-a", "idle_expired", floor - 6 * DAY, Some(floor - 5 * DAY)));
    let d = tenp::insert(&conn, &disabled("tenp-d", "idle_expired", floor + DAY, None));
    let b = tenp::insert(&conn, &disabled("tenp-b", "self_offboard", floor, Some(floor + 3 * DAY)));
    let e = tenp::insert(&conn, &disabled("tenp-e", "self_offboard", floor, Some(floor + 3 * DAY)));
    event(&conn, e, floor + DAY);
    let c = tenp::insert(&conn, &disabled("tenp-c", "idle_expired", floor - 2 * DAY, Some(floor - DAY)));
    event(&conn, c, floor + 20 * DAY);

    let steps: [(i64, &[i64], &[i64]); 8] = [
        // (clock offset from the floor in seconds, rows that must exist, rows that must be gone)
        (14 * DAY - 1, &[a, b, c, d, e], &[]),
        (14 * DAY, &[b, c, d, e], &[a]),
        (15 * DAY - 1, &[b, c, d, e], &[a]),
        (15 * DAY, &[b, c, e], &[a, d]),
        (17 * DAY - 1, &[b, c, e], &[a, d]),
        (17 * DAY, &[c], &[a, b, d, e]),
        (34 * DAY - 1, &[c], &[a, b, d, e]),
        (34 * DAY, &[], &[a, b, c, d, e]),
    ];
    for (offset, exist, gone) in steps {
        let block = sweep(&server, floor + offset).await;
        for id in exist {
            assert!(tenp::tenant_exists(&conn, *id), "clock floor+{offset}s: row {id} is not due yet: {block}");
        }
        for id in gone {
            assert!(!tenp::tenant_exists(&conn, *id), "clock floor+{offset}s: row {id} is due: {block}");
        }
    }
    assert_eq!(purge_audit(&conn).len(), 5, "one audit row each, never twice");
}

/// A subject with `rows` padded `tenant_state_kv` rows: pages the delete frees.
fn bulky_subject(conn: &Connection, ns: &str, last_seen: i64, rows: i64) -> i64 {
    let id = tenp::insert(conn, &disabled(ns, "idle_expired", last_seen - DAY, Some(last_seen)));
    conn.execute(
        "WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < ?2) \
         INSERT INTO tenant_state_kv (tenant_id, key, value_json, updated_unix) \
         SELECT ?1, 'k' || i, printf('%.2000c', 'x'), 1 FROM n",
        params![id, rows],
    )
    .unwrap();
    id
}

#[tokio::test]
async fn tenp_ac05_each_batch_ends_in_a_bounded_incremental_vacuum_never_a_full_vacuum() {
    let server = TestServer::start().await;
    server.state.db.seed_retention_policy_from_env().await.expect("seed");
    let conn = tenp::open_raw(&server.data_dir.0);
    let floor = mcphost::state::now_unix() - 100 * DAY;
    crate::common::tenx::set_floor(&conn, floor);
    tenp::arm_gate(&conn, floor);
    let pages = |sql: &str| tenp::count(&conn, sql);
    let limit = mcphost::retention::PURGE_VACUUM_PAGES;

    // ~16 MiB of state: four times the pages one batch may return.
    for i in 0..4 {
        bulky_subject(&conn, &format!("tenp-bulk-{i}"), floor, 2_000);
    }
    let before = pages("PRAGMA page_count");
    assert_eq!(pages("PRAGMA freelist_count"), 0);
    let block = sweep(&server, floor + 20 * DAY).await;
    assert_eq!(block["purged"], 4, "{block}");
    assert_eq!(block["vacuumed_pages"], limit, "the batch returned exactly PURGE_VACUUM_PAGES: {block}");
    let free = pages("PRAGMA freelist_count");
    assert_eq!(block["freelist_pages"], free, "{block}");
    assert!(free > 0, "the rest of the freed pages stays on the freelist: {free}");
    let after = pages("PRAGMA page_count");
    // The file shrank by the pages the batch vacuumed (a few pointer-map pages aside), and
    // the freelist still holds pages: it did not shrink to its minimum, as a full VACUUM would.
    let shrank = before - after;
    assert!((shrank - limit).abs() <= 64, "the file shrank by about {limit} pages: {before} -> {after}");
    assert!(free > 1_000, "a full VACUUM would have emptied the freelist: {free}");

    // The sweep's own closing vacuum is bounded the same way (it used to drain the whole freelist).
    let again = sweep(&server, floor + 21 * DAY).await;
    assert_eq!(again["purged"], 0);
    let later = pages("PRAGMA freelist_count");
    assert!(free - later <= limit, "one sweep returns at most {limit} pages: {free} -> {later}");
    assert!(later < free, "and it keeps returning them");

    // No full VACUUM statement anywhere in the purge or the sweep.
    for (name, source) in [
        ("src/db/synthetic_purge.rs", include_str!("../src/db/synthetic_purge.rs")),
        ("src/retention.rs", include_str!("../src/retention.rs")),
    ] {
        assert!(source.contains("incremental_vacuum("), "{name} bounds its vacuum");
        for (n, line) in source.lines().enumerate() {
            let code = line.split("//").next().unwrap_or("");
            assert!(
                !code.contains("\"VACUUM") && !code.contains("PRAGMA incremental_vacuum;") && !code.contains(" VACUUM;"),
                "{name}:{}: no unbounded or full vacuum: {line}",
                n + 1
            );
        }
    }
}
