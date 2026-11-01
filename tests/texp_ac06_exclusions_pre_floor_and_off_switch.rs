//! PRD-mcphost-external-unclaimed-tenant-expiry
//! AC6 (P0) — Given fixtures idle 60 days carrying each `EXT` exclusion
//! marker, a pre-floor fixture P with `floor_unix` 1 day ago, and a subject
//! under env var `off`, When the sweep runs with every stage due, Then no
//! fixture gains an event, email, flag or deletion, `prune_log` reports
//! `pre_floor.count = 1` with P in `dry_run`, and a second sweep with
//! `floor_unix` 20 days ago gives P one `warned` or `warn_no_address` event
//! and no `invalidated` one.

use crate::common::texp::{self, DAY};
use crate::common::{EnvGuard, signup};
use mcphost::email::FakeEmailClient;
use mcphost::kinds::KindRegistry;
use serde_json::Value;
use std::sync::Arc;

/// Each exclusion marker: a label and the SQL that adds it to an `EXT` row.
const MARKERS: &[(&str, &str)] = &[
    ("disabled", "disabled = 1, disabled_reason = 'admin_disable'"),
    ("origin-synthetic", "origin = 'synthetic'"),
    ("synthetic-label", "synthetic = 'synthorg:test'"),
    ("owner-verified", "owner_verified_at = created_unix, owner_email = 'm@example.com'"),
    ("claimed", "claimed_unix = created_unix"),
    ("paid", "paid_unix = created_unix"),
    ("billing-ref", "billing_ref = 'sub_test'"),
    ("stripe-customer", "stripe_customer_id = 'cus_test'"),
    ("plan-pro", "plan = 'pro'"),
    ("claimed-and-expired", "owner_verified_at = created_unix, claimed_unix = created_unix, disabled = 1, disabled_reason = 'unclaimed_expired'"),
];

fn last_block(conn: &rusqlite::Connection) -> Option<Value> {
    let json: String = conn.query_row("SELECT deleted_json FROM prune_log ORDER BY id DESC LIMIT 1", [], |r| r.get(0)).ok()?;
    serde_json::from_str::<Value>(&json).ok()?.get("external_unclaimed_tenants").cloned()
}

#[tokio::test]
async fn texp_ac06_markers_are_never_touched_pre_floor_is_listed_and_off_stops_the_sweep() {
    let guard = EnvGuard::clear(texp::ENV).await;
    let email = Arc::new(FakeEmailClient::new());
    let server = texp::server(email.clone(), KindRegistry::with_builtin()).await;
    server.state.db.seed_retention_policy_from_env().await.expect("seed");
    let conn = rusqlite::Connection::open(server.data_dir.0.join("mcphost.db")).expect("raw db");
    let now = mcphost::state::now_unix();
    texp::set_floor(&conn, now - DAY);

    // Markers: idle 60 days, each with a claim attempt, a 11-day-old `warned`
    // event (so an invalidation would be due) and, for the expired one, an
    // old `invalidated` event (so a purge would be due).
    let mut fixtures = Vec::new();
    for (i, (label, sql)) in MARKERS.iter().enumerate() {
        let (ns, _key) = signup(&server.base_url, &format!("Texp marker {label}")).await;
        let id = texp::make_external(&conn, &ns, now, 70, Some(60));
        conn.execute(&format!("UPDATE tenants SET {sql} WHERE id = ?1"), [id]).unwrap();
        texp::add_claim_attempt(&conn, id, &format!("marker{i}@example.com"), now - 30 * DAY);
        texp::add_event(&conn, id, "warned", now - 60 * DAY, now - 11 * DAY);
        if *label == "claimed-and-expired" {
            texp::add_event(&conn, id, "invalidated", now - 60 * DAY, now - 20 * DAY);
        }
        fixtures.push(id);
    }
    let seeded_events: Vec<i64> = fixtures.iter().map(|id| texp::stages(&conn, *id).len() as i64).collect();
    // P: external, unclaimed, minted before the floor, never seen: its clock starts at the floor.
    let (p_ns, _) = signup(&server.base_url, "Texp pre-floor P").await;
    let p = texp::make_external(&conn, &p_ns, now, 70, None);

    // Sweep 1: floor 1 day ago, so nothing is due; the pre-floor list is written in full.
    server.state.db.prune_once().await.expect("prune_once 1");
    let block = last_block(&conn).expect("external block in prune_log");
    assert_eq!(block["pre_floor"]["count"], 1, "only P is a pre-floor subject: {block}");
    let listed: Vec<i64> = block["pre_floor"]["dry_run"].as_array().expect("dry_run").iter().map(|r| r["tenant_id"].as_i64().unwrap()).collect();
    assert_eq!(listed, vec![p], "P is in dry_run");
    assert_eq!(block["pre_floor"]["warn_due_unix"], now - DAY + 20 * DAY);
    assert_eq!(block["pre_floor"]["invalidate_due_unix"], now - DAY + 30 * DAY);
    assert_eq!(block["pre_floor"]["purge_due_unix"], now - DAY + 44 * DAY);
    assert!(texp::stages(&conn, p).is_empty(), "nothing is due on the deploy day");

    // The off switch: P is a subject once its clock is due, but `off` stops every stage before any SQL.
    texp::set_floor(&conn, now - 20 * DAY);
    drop(guard);
    {
        let _off = EnvGuard::set(texp::ENV, "off").await;
        server.state.db.prune_once().await.expect("prune_once off");
        assert!(last_block(&conn).is_none(), "an off policy writes no block");
        assert!(texp::stages(&conn, p).is_empty(), "off: no event");
        assert_eq!(email.send_count(), 0, "off: no email");
    }

    // Sweep 2: floor 20 days ago. P is due a warning; no marker is.
    let _clear = EnvGuard::clear(texp::ENV).await;
    server.state.db.prune_once().await.expect("prune_once 2");
    let block = last_block(&conn).expect("block");
    assert!(block["pre_floor"].get("dry_run").is_none(), "later sweeps carry count and stamps only: {block}");
    assert_eq!(block["pre_floor"]["count"], 1);
    let p_events = texp::stages(&conn, p);
    assert_eq!(p_events.len(), 1, "P gets one event: {p_events:?}");
    assert!(p_events[0] == "warned" || p_events[0] == "warn_no_address", "{p_events:?}");
    assert_eq!(texp::count_stage(&conn, p, "invalidated"), 0);

    // And on a floor 40 days back an invalidation and a purge are both due -- for subjects only.
    texp::set_floor(&conn, now - 40 * DAY);
    server.state.db.prune_once().await.expect("prune_once 3");
    for (id, before) in fixtures.iter().zip(&seeded_events) {
        assert_eq!(texp::stages(&conn, *id).len() as i64, *before, "marker {id} gained no event");
    }
    assert!(
        email.sends().iter().all(|m| !m.to.starts_with("marker")),
        "no marker fixture was emailed: {:?}",
        email.sends().iter().map(|m| m.to.clone()).collect::<Vec<_>>()
    );
    let ids: Vec<String> = fixtures.iter().map(i64::to_string).collect();
    let kept: i64 = conn.query_row(&format!("SELECT COUNT(*) FROM tenants WHERE id IN ({})", ids.join(",")), [], |r| r.get(0)).unwrap();
    assert_eq!(kept, fixtures.len() as i64, "no fixture was deleted");
    for (i, id) in fixtures.iter().enumerate() {
        let (disabled, reason) = texp::state(&conn, *id);
        let expected = match MARKERS[i].0 {
            "disabled" => (true, Some("admin_disable".to_string())),
            "claimed-and-expired" => (true, Some("unclaimed_expired".to_string())),
            _ => (false, None),
        };
        assert_eq!((disabled, reason), expected, "marker {} keeps its state", MARKERS[i].0);
    }
}
