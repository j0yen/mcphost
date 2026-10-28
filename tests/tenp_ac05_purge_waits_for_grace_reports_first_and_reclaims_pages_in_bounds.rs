//! PRD-mcphost-synthetic-tenant-purge
//! AC5 (P0) — Given two-marker rows disabled idle_expired or self_offboard, When sweeps run on a mock
//! clock, Then none is deleted before MAX(last_seen or created, last lifecycle event, floor) + grace,
//! a sweep with no prior prune_log row carrying purge_due_unix only reports would_purge and
//! cross_refs, later sweeps delete via delete_tenant_as logging tenant_purge_synthetic; each batch
//! ends in incremental_vacuum(PURGE_VACUUM_PAGES), no VACUUM.

use crate::common::tenp::{self, DAY, Planter, Row};
use crate::common::{EnvGuard, TestServer};
use rusqlite::{Connection, params};
use rusqlite::types::Value as Sql;

const GRACE: i64 = 14 * DAY;

async fn at(server: &TestServer, secs: i64) {
    server.state.db.prune_once_at(secs).await.expect("sweep");
}

fn audit(conn: &Connection) -> Vec<String> {
    conn.prepare("SELECT tenant FROM admin_events WHERE action = 'tenant_purge_synthetic' ORDER BY id")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap()
}

#[tokio::test]
async fn tenp_ac05_no_row_is_deleted_before_the_latest_of_last_seen_event_and_floor_plus_grace() {
    let _env = EnvGuard::clear(tenp::BUDGET_ENV).await;
    let server = TestServer::start().await;
    server.state.db.seed_retention_policy_from_env().await.expect("seed");
    let conn = Connection::open(server.data_dir.0.join("mcphost.db")).expect("raw db");
    let t0 = mcphost::state::now_unix();
    let floor = t0 - 5 * DAY;
    tenp::set_floor(&conn, floor);

    // Last seen before the floor: the floor starts the grace.
    let a = tenp::insert(&conn, &Row::purge_subject("tenp5-a", t0, "idle_expired", 60, 20));
    // Last seen after the floor: the last-seen starts it.
    let b = tenp::insert(&conn, &Row::purge_subject("tenp5-b", t0, "self_offboard", 60, 1));
    // An event newer than both starts it.
    let c = tenp::insert(&conn, &Row::purge_subject("tenp5-c", t0, "idle_expired", 60, 30));
    conn.execute(
        "INSERT INTO tenant_lifecycle_events (tenant_id, policy, stage, idle_since_unix, created_unix) \
         VALUES (?1, 'synthetic_implicit_tenants', 'invalidated', ?2, ?3)",
        params![c, t0 - 30 * DAY, t0 - 2 * DAY],
    )
    .unwrap();
    // Never subjects: a perf fixture, an admin disable, an enabled explicit sign-up.
    let perf = tenp::insert(&conn, &Row::purge_subject("perf-free", t0, "idle_expired", 60, 30));
    let admin = tenp::insert(&conn, &Row::purge_subject("tenp5-admin", t0, "admin_disable", 60, 30));
    let live = tenp::insert(&conn, &Row { signup_source: Some("explicit"), disabled: false, disabled_reason: None, ..Row::purge_subject("tenp5-live", t0, "idle_expired", 60, 30) });
    let (due_a, due_c, due_b) = (floor + GRACE, t0 - 2 * DAY + GRACE, t0 - DAY + GRACE);
    assert_eq!((due_a, due_c, due_b), (t0 + 9 * DAY, t0 + 12 * DAY, t0 + 13 * DAY));

    // The sweep that finds no earlier row with `purge_due_unix` reports it and deletes nothing.
    server.state.db.prune_once_at(t0).await.expect("first sweep");
    let first = tenp::last_block(&conn);
    assert_eq!((first["purged"].as_i64(), first["would_purge"].as_i64()), (Some(0), Some(0)), "{first}");
    assert_eq!(first["purge_due_unix"], due_a, "the earliest due stamp: {first}");

    // One second early: nothing is due; on the second: A goes, exactly one row.
    at(&server, due_a - 1).await;
    assert_eq!(tenp::last_block(&conn)["would_purge"], 0);
    assert!(tenp::exists(&conn, a));
    at(&server, due_a).await;
    let block = tenp::last_block(&conn);
    assert_eq!((block["purged"].as_i64(), block["would_purge"].as_i64()), (Some(1), Some(1)), "{block}");
    assert!(!tenp::exists(&conn, a), "A is purged at its due stamp");
    assert_eq!(audit(&conn), ["tenp5-a"], "the purge journals tenant_purge_synthetic");
    assert!(tenp::exists(&conn, b) && tenp::exists(&conn, c));

    at(&server, due_c - 1).await;
    assert!(tenp::exists(&conn, c), "the lifecycle event moved C's date past its last-seen");
    at(&server, due_c).await;
    assert!(!tenp::exists(&conn, c));
    at(&server, due_b - 1).await;
    assert!(tenp::exists(&conn, b));
    at(&server, due_b).await;
    assert!(!tenp::exists(&conn, b), "B (self_offboard) is purged 14 days after its last-seen");
    assert_eq!(audit(&conn), ["tenp5-a", "tenp5-c", "tenp5-b"]);
    for id in [perf, admin, live] {
        assert!(tenp::exists(&conn, id), "{id} is never a subject");
    }
    at(&server, due_b + 400 * DAY).await;
    let end = tenp::last_block(&conn);
    assert_eq!((end["purged"].as_i64(), end["would_purge"].as_i64()), (Some(0), Some(0)), "{end}");
    assert!(end["purge_due_unix"].is_null(), "no purge-class row is left: {end}");
}

#[tokio::test]
async fn tenp_ac05_a_sweep_with_no_armed_predecessor_reports_would_purge_and_cross_refs_only() {
    let _env = EnvGuard::clear(tenp::BUDGET_ENV).await;
    let server = TestServer::start().await;
    server.state.db.seed_retention_policy_from_env().await.expect("seed");
    let conn = Connection::open(server.data_dir.0.join("mcphost.db")).expect("raw db");
    conn.execute_batch("PRAGMA foreign_keys = OFF").unwrap();
    let t0 = mcphost::state::now_unix();
    tenp::set_floor(&conn, t0 - 40 * DAY);
    let planter = Planter::new(&conn, t0 - 3600);
    let subject = tenp::insert(&conn, &Row::purge_subject("tenp5-report", t0, "idle_expired", 60, 30));
    let owner = tenp::insert(&conn, &Row::implicit("tenp5-report-owner", t0, 5, 3600));
    planter.plant_with(&conn, "calls", "tenant_id", owner, 1, &[("caller_tenant_id", Sql::Integer(subject))]).unwrap();
    // A prior row exists, but the policy block it carries has no `purge_due_unix` (and the external
    // block's own `pre_floor.purge_due_unix` must not arm the gate).
    tenp::seed_pre_prd_log_row(&conn, t0 - 3600);

    server.state.db.prune_once_at(t0).await.expect("report-only sweep");
    let first = tenp::last_block(&conn);
    assert_eq!(first["purged"], 0, "{first}");
    assert_eq!(first["would_purge"], 1, "{first}");
    assert_eq!(first["cross_refs"]["calls.caller_tenant_id"]["synthetic_implicit"], 1, "{first}");
    assert_eq!(first["cross_refs"]["calls.caller_tenant_id"]["outcome"], "nulled", "{first}");
    assert!(tenp::exists(&conn, subject), "nothing is deleted by the sweep that carries the purge block first");
    let named: i64 = conn.query_row("SELECT COUNT(*) FROM calls WHERE caller_tenant_id = ?1", params![subject], |r| r.get(0)).unwrap();
    assert_eq!(named, 1, "no cross reference is changed either");

    server.state.db.prune_once_at(t0).await.expect("purging sweep");
    let second = tenp::last_block(&conn);
    assert_eq!(second["purged"], 1, "{second}");
    assert!(!tenp::exists(&conn, subject));
    let nulled: i64 = conn.query_row("SELECT COUNT(*) FROM calls WHERE tenant_id = ?1 AND caller_tenant_id IS NULL", params![owner], |r| r.get(0)).unwrap();
    assert_eq!(nulled, 1, "the other tenant's call row survives with the caller NULL");
    let detail: String = conn
        .query_row("SELECT detail FROM admin_events WHERE action = 'tenant_purge_synthetic'", [], |r| r.get(0))
        .unwrap();
    assert!(detail.contains("\"calls.caller_tenant_id\""), "the journal row names the cross reference it cleared: {detail}");
}

#[tokio::test]
async fn tenp_ac05_each_batch_ends_in_a_bounded_incremental_vacuum() {
    let _env = EnvGuard::clear(tenp::BUDGET_ENV).await;
    let server = TestServer::start().await;
    server.state.db.seed_retention_policy_from_env().await.expect("seed");
    let mut conn = Connection::open(server.data_dir.0.join("mcphost.db")).expect("raw db");
    conn.execute_batch("PRAGMA foreign_keys = OFF").unwrap();
    let auto_vacuum: i64 = conn.query_row("PRAGMA auto_vacuum", [], |r| r.get(0)).unwrap();
    assert_eq!(auto_vacuum, 2, "the database is in incremental auto-vacuum mode");
    let t0 = mcphost::state::now_unix();
    tenp::set_floor(&conn, t0 - 40 * DAY);
    let planter = Planter::new(&conn, t0 - 3600);
    let subject = tenp::insert(&conn, &Row::purge_subject("tenp5-pages", t0, "idle_expired", 60, 30));
    // About 14 MB of state: well over PURGE_VACUUM_PAGES (2,048 pages, 8 MiB) once deleted.
    let tx = conn.transaction().unwrap();
    let big = "x".repeat(2_000);
    for k in 0..7_000 {
        planter.plant_with(&tx, "tenant_state_kv", "tenant_id", subject, k, &[("value_json", Sql::Text(big.clone()))]).unwrap();
    }
    tx.commit().unwrap();

    server.state.db.prune_once_at(t0).await.expect("report-only sweep");
    let report = tenp::last_block(&conn);
    assert_eq!(report["vacuumed_pages"], 0, "the report-only sweep reclaims nothing: {report}");
    let pages_before: i64 = conn.query_row("PRAGMA page_count", [], |r| r.get(0)).unwrap();
    server.state.db.prune_once_at(t0).await.expect("purging sweep");
    let block = tenp::last_block(&conn);
    assert_eq!(block["purged"], 1, "{block}");
    let vacuumed = block["vacuumed_pages"].as_i64().unwrap();
    let freelist = block["freelist_pages"].as_i64().unwrap();
    assert_eq!(vacuumed, 2_048, "the batch reclaims PURGE_VACUUM_PAGES pages and no more: {block}");
    assert!(freelist > 0, "the rest of the freed pages wait on the freelist for reuse: {block}");
    assert_eq!(block["batch_wall_ms"].as_array().unwrap().len(), 1, "one batch: {block}");
    let (pages_after, free_now): (i64, i64) = (
        conn.query_row("PRAGMA page_count", [], |r| r.get(0)).unwrap(),
        conn.query_row("PRAGMA freelist_count", [], |r| r.get(0)).unwrap(),
    );
    assert_eq!(free_now, freelist, "the block reports the freelist the database has");
    // The file shrinks by the reclaimed pages plus the few pointer-map pages that go with them.
    assert!(pages_before - pages_after <= 2_048 + 8, "the file shrank by about one batch's pages: {pages_before} -> {pages_after}");
}
