//! PRD-mcphost-synthetic-tenant-purge
//! AC5 (P0) — Given two-marker rows disabled idle_expired or self_offboard,
//! When sweeps run on a mock clock, Then none is deleted before MAX(last_seen
//! or created, last lifecycle event, floor) + grace, a sweep with no prior
//! prune_log row carrying purge_due_unix only reports would_purge and
//! cross_refs, later sweeps delete via delete_tenant_as logging
//! tenant_purge_synthetic; each batch ends in incremental_vacuum(
//! PURGE_VACUUM_PAGES), no VACUUM.
//!
//! The clock is `Db::prune_once_at`: every stage dates its work from the
//! caller's `now`, so the test walks one database across a month of sweeps.

use crate::common::TestServer;
use crate::common::tenp::{self, DAY, Kind};
use rusqlite::Connection;
use serde_json::Value;

const GRACE: i64 = 14 * DAY;

fn purged(conn: &Connection) -> Vec<String> {
    let mut stmt = conn
        .prepare("SELECT tenant FROM admin_events WHERE action = 'tenant_purge_synthetic' ORDER BY id")
        .unwrap();
    stmt.query_map([], |r| r.get(0)).unwrap().collect::<Result<_, _>>().unwrap()
}

fn present(conn: &Connection, ns: &str) -> bool {
    tenp::count(conn, &format!("SELECT COUNT(*) FROM tenants WHERE namespace = '{ns}'")) == 1
}

fn id_of(conn: &Connection, ns: &str) -> i64 {
    conn.query_row("SELECT id FROM tenants WHERE namespace = ?1", [ns], |r| r.get(0)).unwrap()
}

/// One synthetic tenant of `kind` naming each of `subjects` as a contact, so the sweep has `cross_refs` to count.
fn name_subjects(conn: &Connection, now: i64, kind: Kind, subjects: &[&str]) {
    let cp = tenp::insert_tenant(conn, "tenp-cp-syn", kind, now);
    for ns in subjects {
        let s = id_of(conn, ns).to_string();
        tenp::plant(conn, "contacts", &[("tenant_id", cp.to_string()), ("contact_tenant_id", s)]);
    }
}

#[tokio::test]
async fn tenp_ac05_a_subject_is_deleted_on_its_due_day_and_not_before() {
    let server = TestServer::start().await;
    let db = &server.state.db;
    db.seed_retention_policy_from_env().await.expect("seed");
    let conn = Connection::open(server.data_dir.0.join("mcphost.db")).expect("raw db");
    conn.busy_timeout(std::time::Duration::from_secs(30)).expect("busy timeout");
    let real_now = mcphost::state::now_unix();
    let floor = real_now - 3 * DAY;
    tenp::set_floor(&conn, floor);
    tenp::widen_row_retention(&conn);

    // Four subjects with four different clocks, and a claimed one that is never a subject.
    // a: last seen before the floor -> the floor dates it.
    tenp::insert_subjects(&conn, "tenp-a-", 1, floor - 40 * DAY, floor - 10 * DAY, "idle_expired");
    // b: last seen after the floor.
    tenp::insert_subjects(&conn, "tenp-b-", 1, floor - 40 * DAY, floor + 2 * DAY, "self_offboard");
    // c: last seen before the floor, but a lifecycle event after it.
    tenp::insert_subjects(&conn, "tenp-c-", 1, floor - 40 * DAY, floor - 10 * DAY, "idle_expired");
    conn.execute(
        "INSERT INTO tenant_lifecycle_events (tenant_id, policy, stage, idle_since_unix, created_unix) \
         VALUES (?1, ?2, 'invalidated', ?3, ?4)",
        rusqlite::params![id_of(&conn, "tenp-c-1"), tenp::POLICY, floor - 10 * DAY, floor + 5 * DAY],
    )
    .unwrap();
    // d: created after the floor, last seen then too.
    tenp::insert_subjects(&conn, "tenp-d-", 1, floor + DAY, floor + DAY, "self_offboard");
    // e: a claimed test account that disabled itself.
    tenp::insert_subjects(&conn, "tenp-e-", 1, floor - 40 * DAY, floor - 10 * DAY, "self_offboard");
    conn.execute("UPDATE tenants SET claimed_unix = ?1 WHERE namespace = 'tenp-e-1'", [floor - 20 * DAY]).unwrap();
    let all = ["tenp-a-1", "tenp-b-1", "tenp-c-1", "tenp-d-1"];
    // An explicit sign-up never idles out, so it stays on the mock clock's month-long walk.
    name_subjects(&conn, real_now, Kind::SyntheticExplicit, &all);
    for ns in all.iter().chain(&["tenp-e-1"]) {
        tenp::plant(&conn, "tenant_state_kv", &[("tenant_id", id_of(&conn, ns).to_string())]);
    }

    let due = |ns: &str| match ns {
        "tenp-a-1" => floor + GRACE,
        "tenp-b-1" => floor + 2 * DAY + GRACE,
        "tenp-c-1" => floor + 5 * DAY + GRACE,
        "tenp-d-1" => floor + DAY + GRACE,
        _ => unreachable!(),
    };
    let sweep = |at: i64| async move { db.prune_once_at(at).await.expect("sweep").synthetic };

    // The first sweep that carries the block arms the gate and reports the earliest due stamp.
    let block = sweep(floor + DAY).await;
    assert_eq!(block["purge_due_unix"], due("tenp-a-1"), "{block}");
    assert_eq!((block["purged"].as_i64(), block["would_purge"].as_i64()), (Some(0), Some(0)), "{block}");

    // The walk: (sweep clock, subjects gone afterwards).
    let walk: [(i64, &[&str]); 6] = [
        (due("tenp-a-1") - 1, &[]),
        (due("tenp-a-1"), &["tenp-a-1"]),
        (due("tenp-d-1"), &["tenp-a-1", "tenp-d-1"]),
        (due("tenp-c-1") - 1, &["tenp-a-1", "tenp-d-1", "tenp-b-1"]),
        (due("tenp-c-1"), &["tenp-a-1", "tenp-d-1", "tenp-b-1", "tenp-c-1"]),
        (due("tenp-c-1") + 30 * DAY, &["tenp-a-1", "tenp-d-1", "tenp-b-1", "tenp-c-1"]),
    ];
    for (at, gone) in walk {
        let block = sweep(at).await;
        for ns in all {
            assert_eq!(present(&conn, ns), !gone.contains(&ns), "{ns} at {at} ({} days after the floor): {block}", (at - floor) / DAY);
        }
        // `b` is due before `c` and after `d`, so the mock-clock steps above name the order the due dates give.
        assert_eq!(purged(&conn).len(), gone.len(), "one tenant_purge_synthetic row per deleted tenant ({:?}): {block}", purged(&conn));
        assert!(present(&conn, "tenp-e-1"), "a claimed tenant is never purged");
    }
    assert_eq!(purged(&conn), ["tenp-a-1", "tenp-d-1", "tenp-b-1", "tenp-c-1"], "deleted in due order, each by its own sweep");
    // The journal line carries the subject's counts and what named it.
    let detail: String = conn
        .query_row("SELECT detail FROM admin_events WHERE action = 'tenant_purge_synthetic' AND tenant = 'tenp-a-1'", [], |r| r.get(0))
        .unwrap();
    let detail: Value = serde_json::from_str(&detail).unwrap();
    assert_eq!(detail["cross_refs"]["contacts.contact_tenant_id"]["outcome"], "cascade", "{detail}");
    assert_eq!(tenp::count(&conn, "SELECT COUNT(*) FROM tenant_state_kv WHERE tenant_id NOT IN (SELECT id FROM tenants)"), 0);
}

#[tokio::test]
async fn tenp_ac05_the_first_sweep_that_carries_the_block_only_reports() {
    let server = TestServer::start().await;
    let db = &server.state.db;
    db.seed_retention_policy_from_env().await.expect("seed");
    let conn = Connection::open(server.data_dir.0.join("mcphost.db")).expect("raw db");
    // The fixture plants child rows without their parents.
    conn.pragma_update(None, "foreign_keys", "OFF").expect("foreign_keys off");
    let now = mcphost::state::now_unix();
    tenp::set_floor(&conn, now - 30 * DAY);
    tenp::widen_row_retention(&conn);
    // Three subjects due since the floor + grace, a prior sweep row of the pre-PRD shape (the policy
    // block present, no `purge_due_unix`) and a counterpart naming each of them.
    tenp::insert_subjects(&conn, "tenp-first-", 3, now - 60 * DAY, now - 20 * DAY, "self_offboard");
    tenp::seed_pre_prd_prune_log(&conn, now - 5 * DAY);
    name_subjects(&conn, now, Kind::SyntheticImplicit, &["tenp-first-1", "tenp-first-2", "tenp-first-3"]);
    tenp::fill_children(&conn, "tenp-first-%", &[]);

    let before = tenp::dump(&conn, &["prune_log", "blobs", "call_results"]);
    let first = db.prune_once_at(now).await.expect("first sweep").synthetic;
    assert_eq!(first["purged"], 0, "{first}");
    assert_eq!(first["would_purge"], 3, "{first}");
    assert!(first["purge_due_unix"].is_i64(), "{first}");
    let named = &first["cross_refs"]["contacts.contact_tenant_id"];
    assert_eq!((named["synthetic_implicit"].as_i64(), named["other"].as_i64()), (Some(3), Some(0)), "{first}");
    tenp::assert_same_dump(&before, &tenp::dump(&conn, &["prune_log", "blobs", "call_results"]), "the report-only sweep deleted or changed a row");
    assert!(purged(&conn).is_empty());

    // The next sweep sees the token in the first one's row and deletes.
    let second = db.prune_once_at(now).await.expect("second sweep").synthetic;
    assert_eq!(second["purged"], 3, "{second}");
    assert_eq!(purged(&conn).len(), 3);
    assert_eq!(tenp::count(&conn, "SELECT COUNT(*) FROM tenants WHERE namespace LIKE 'tenp-first-%'"), 0);
}

/// Every whole word `vacuum` in the code of `source` (comments aside) is the tail of
/// `incremental_vacuum` or of the `auto_vacuum` pragma.
fn only_incremental_vacuum(source: &str) -> bool {
    let code: String = source.lines().filter(|l| !l.trim_start().starts_with("//")).collect::<Vec<_>>().join("\n");
    let lower = code.to_ascii_lowercase();
    lower.match_indices("vacuum").all(|(at, word)| {
        let after = lower[at + word.len()..].chars().next();
        let whole_word = after.is_none_or(|c| !(c.is_ascii_alphanumeric() || c == '_'));
        !whole_word || lower[..at].ends_with("incremental_") || lower[..at].ends_with("auto_")
    })
}

#[tokio::test]
async fn tenp_ac05_each_batch_ends_in_a_bounded_incremental_vacuum_and_never_a_vacuum() {
    let server = TestServer::start().await;
    let db = &server.state.db;
    db.seed_retention_policy_from_env().await.expect("seed");
    let conn = Connection::open(server.data_dir.0.join("mcphost.db")).expect("raw db");
    let now = mcphost::state::now_unix();
    tenp::set_floor(&conn, now - 30 * DAY);
    tenp::widen_row_retention(&conn);
    tenp::insert_subjects(&conn, "tenp-big-", 1, now - 60 * DAY, now - 20 * DAY, "self_offboard");
    let id = id_of(&conn, "tenp-big-1");
    // About 16 MB of state: far more than one bounded vacuum hands back.
    for i in 0..16 {
        conn.execute(
            "INSERT INTO tenant_state_kv (tenant_id, key, value_json, updated_unix) VALUES (?1, ?2, hex(zeroblob(500000)), 1)",
            rusqlite::params![id, format!("k{i}")],
        )
        .unwrap();
    }
    assert_eq!(tenp::count(&conn, "PRAGMA auto_vacuum"), 2, "incremental auto_vacuum");
    let pages = tenp::count(&conn, "PRAGMA page_count");

    db.prune_once_at(now).await.expect("first sweep");
    assert!(present(&conn, "tenp-big-1"));
    let block = db.prune_once_at(now).await.expect("second sweep").synthetic;
    assert_eq!(block["purged"], 1, "{block}");
    let limit = mcphost::retention::PURGE_VACUUM_PAGES;
    assert_eq!(block["vacuumed_pages"], limit, "the batch hands back exactly the bound: {block}");
    let freelist = block["freelist_pages"].as_i64().expect("freelist_pages");
    assert!(freelist > 0, "a full VACUUM would leave none; the rest is left for later writes: {block}");
    assert!(block["batches"][0]["wall_ms"].is_u64(), "per-batch wall time: {block}");
    let left = tenp::count(&conn, "PRAGMA freelist_count");
    assert!(left > 0 && left <= freelist, "{left} pages still free on disk");
    let after = tenp::count(&conn, "PRAGMA page_count");
    assert!(after >= pages - limit - 16, "the file shrank by at most the bound (plus pointer-map pages): {pages} -> {after}");

    for (name, source) in [
        ("src/db/purge.rs", include_str!("../src/db/purge.rs")),
        ("src/retention.rs", include_str!("../src/retention.rs")),
    ] {
        assert!(only_incremental_vacuum(source), "{name} names a VACUUM that is not incremental_vacuum");
    }
}
