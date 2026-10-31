//! PRD-mcphost-synthetic-tenant-purge
//! AC7 (P0) — Given the § Classes rows, When sweeps pass the window and grace,
//! Then only the both-markers unclaimed row gains event, flag and deletion, the
//! other eight none, even if a class flips between mark and delete; grace env off
//! purges nothing, idle-days env off disables or purges nothing; a disabled
//! subject's old tenant_key gets tenant_key_invalid and moves no due date, and a
//! keyless new peer still mints, never a purged namespace.
//!
//! Sweeps run on `Db::prune_once_at`'s mock clock. Every test holds an `EnvGuard`
//! from before its server starts: the policy reads the process environment, and the
//! idle-days test sets it.

use crate::common::tenp::{self, DAY, GRACE_ENV, IDLE_ENV, POLICY};
use crate::common::tenx::{self, Fx};
use crate::common::{ADMIN_KEY, EnvGuard, McpClient, TestServer, extract_structured, signup};
use mcphost::auth::TEST_NAMESPACE_CANDIDATES_ENV;
use rusqlite::Connection;
use serde_json::json;

const GRACE: i64 = 14 * DAY;

fn id_of(conn: &Connection, ns: &str) -> i64 {
    conn.query_row("SELECT id FROM tenants WHERE namespace = ?1", [ns], |r| r.get(0)).unwrap()
}

fn exists(conn: &Connection, ns: &str) -> bool {
    tenp::count(conn, &format!("SELECT COUNT(*) FROM tenants WHERE namespace = '{ns}'")) == 1
}

/// The synthetic policy's lifecycle events of one tenant (the external policy keeps its own).
fn events(conn: &Connection, ns: &str) -> i64 {
    tenp::count(
        conn,
        &format!(
            "SELECT COUNT(*) FROM tenant_lifecycle_events WHERE policy = '{POLICY}' \
             AND tenant_id = (SELECT id FROM tenants WHERE namespace = '{ns}')"
        ),
    )
}

fn purged(conn: &Connection) -> Vec<String> {
    let mut stmt = conn
        .prepare("SELECT tenant FROM admin_events WHERE action = 'tenant_purge_synthetic' ORDER BY id")
        .unwrap();
    stmt.query_map([], |r| r.get(0)).unwrap().collect::<Result<_, _>>().unwrap()
}

/// A seeded server with the floor 30 days back and the plain row windows widened.
async fn server_with_floor() -> (TestServer, Connection, i64) {
    let server = TestServer::start().await;
    server.state.db.seed_retention_policy_from_env().await.expect("seed");
    let conn = Connection::open(server.data_dir.0.join("mcphost.db")).expect("raw db");
    conn.busy_timeout(std::time::Duration::from_secs(30)).expect("busy timeout");
    let now = mcphost::state::now_unix();
    tenp::set_floor(&conn, now - 30 * DAY);
    tenp::widen_row_retention(&conn);
    (server, conn, now)
}

#[tokio::test]
async fn tenp_ac07_only_the_both_markers_unclaimed_row_gains_event_flag_and_deletion() {
    let _env = EnvGuard::clear(GRACE_ENV).await;
    let (server, conn, now) = server_with_floor().await;
    let db = &server.state.db;
    let idle = 5 * DAY;

    // The nine § Classes rows, enabled and idle past the window.
    let mut classes: Vec<(&str, Box<dyn Fn(&mut Fx)>)> = vec![
        ("tenp-cls-both", Box::new(|_| {})),
        ("tenp-cls-label-only", Box::new(|f| f.origin = "external")),
        ("tenp-cls-origin-only", Box::new(|f| f.synthetic = None)),
        ("tenp-cls-neither", Box::new(|f| {
            f.origin = "external";
            f.synthetic = None;
        })),
        ("tenp-cls-claimed", Box::new(move |f| f.claimed_unix = Some(now - DAY))),
        ("tenp-cls-paid", Box::new(move |f| f.paid_unix = Some(now - DAY))),
        ("tenp-cls-billing-ref", Box::new(|f| f.billing_ref = Some("cus_test"))),
        ("tenp-cls-owner-verified", Box::new(|_| {})),
        ("tenp-cls-admin-disable", Box::new(|_| {})),
    ];
    for (ns, mutate) in classes.iter_mut() {
        let mut fx = Fx::subject(ns, now, 20, idle);
        mutate(&mut fx);
        tenx::insert(&conn, &fx);
    }
    conn.execute("UPDATE tenants SET owner_verified_at = ?1 WHERE namespace = 'tenp-cls-owner-verified'", [now - DAY]).unwrap();
    conn.execute(
        "UPDATE tenants SET disabled = 1, disabled_reason = 'admin_disable' WHERE namespace = 'tenp-cls-admin-disable'",
        [],
    )
    .unwrap();
    // Four more subjects whose class flips after the mark and before the delete.
    let flips = ["tenp-flip-claimed", "tenp-flip-paid", "tenp-flip-billing-ref", "tenp-flip-owner-verified"];
    for ns in flips {
        tenx::insert(&conn, &Fx::subject(ns, now, 20, idle));
    }
    let others: Vec<&str> = classes.iter().map(|(ns, _)| *ns).filter(|ns| *ns != "tenp-cls-both").collect();
    let state_of = |ns: &str| tenx::state(&conn, id_of(&conn, ns));
    let before: Vec<_> = others.iter().map(|ns| state_of(ns)).collect();

    // Sweep 1 marks: the subject and the four soon-to-flip rows, nobody else.
    let marked = db.prune_once_at(now).await.expect("mark sweep").synthetic;
    assert_eq!(marked["expired"], 5, "{marked}");
    assert_eq!(state_of("tenp-cls-both"), (true, Some("idle_expired".to_string())));
    assert_eq!(events(&conn, "tenp-cls-both"), 1, "the mark writes one lifecycle event");
    for (ns, was) in others.iter().zip(&before) {
        assert_eq!(&state_of(ns), was, "{ns} is not a subject");
        assert_eq!(events(&conn, ns), 0, "{ns} gains no event");
    }
    // The class flips between mark and delete.
    conn.execute("UPDATE tenants SET claimed_unix = ?1 WHERE namespace = 'tenp-flip-claimed'", [now]).unwrap();
    conn.execute("UPDATE tenants SET paid_unix = ?1 WHERE namespace = 'tenp-flip-paid'", [now]).unwrap();
    conn.execute("UPDATE tenants SET billing_ref = 'cus_late' WHERE namespace = 'tenp-flip-billing-ref'", []).unwrap();
    conn.execute("UPDATE tenants SET owner_verified_at = ?1 WHERE namespace = 'tenp-flip-owner-verified'", [now]).unwrap();

    // Sweep 2 passes the window and the grace: only the unflipped subject goes.
    let swept = db.prune_once_at(now + GRACE + DAY).await.expect("delete sweep").synthetic;
    assert_eq!(swept["purged"], 1, "{swept}");
    assert_eq!(purged(&conn), ["tenp-cls-both"]);
    assert!(!exists(&conn, "tenp-cls-both"));
    for ns in flips {
        assert!(exists(&conn, ns), "{ns} flipped class and stays");
        assert_eq!(events(&conn, ns), 1, "{ns} keeps the one event it had");
    }
    for (ns, was) in others.iter().zip(&before) {
        assert_eq!(&state_of(ns), was, "{ns} is untouched after the delete sweep");
        assert_eq!(events(&conn, ns), 0, "{ns} still has no event");
    }
}

#[tokio::test]
async fn tenp_ac07_grace_env_off_purges_nothing_and_removing_it_still_reports_first() {
    let off = EnvGuard::set(GRACE_ENV, "off").await;
    let (server, conn, now) = server_with_floor().await;
    let db = &server.state.db;
    tenp::insert_subjects(&conn, "tenp-grace-", 3, now - 60 * DAY, now - 40 * DAY, "self_offboard");
    let later = now + GRACE;

    {
        for _ in 0..2 {
            let block = db.prune_once_at(later).await.expect("sweep with the grace off").synthetic;
            assert_eq!(block["purge"], "off", "{block}");
            assert!(block.get("purge_due_unix").is_none(), "an off sweep never writes the token: {block}");
            assert!(block.get("purged").is_none(), "{block}");
            assert_eq!(tenp::count(&conn, "SELECT COUNT(*) FROM tenants WHERE namespace LIKE 'tenp-grace-%'"), 3);
        }
    }
    drop(off);
    let _on = EnvGuard::clear(GRACE_ENV).await;
    // Removing the env yields one report-only sweep before the first delete.
    let report = db.prune_once_at(later).await.expect("first sweep without the env").synthetic;
    assert_eq!((report["purged"].as_i64(), report["would_purge"].as_i64()), (Some(0), Some(3)), "{report}");
    assert_eq!(tenp::count(&conn, "SELECT COUNT(*) FROM tenants WHERE namespace LIKE 'tenp-grace-%'"), 3);
    let deleted = db.prune_once_at(later).await.expect("second sweep without the env").synthetic;
    assert_eq!(deleted["purged"], 3, "{deleted}");
    assert!(purged(&conn).len() == 3);
}

#[tokio::test]
async fn tenp_ac07_idle_days_env_off_disables_and_purges_nothing() {
    for value in ["off", "0"] {
        let _off = EnvGuard::set(IDLE_ENV, value).await;
        let (server, conn, now) = server_with_floor().await;
        let db = &server.state.db;
        // The env is read at start-up: re-seed the stored window under it.
        db.seed_retention_policy_from_env().await.expect("reseed");
        assert_eq!(tenx::stored_days(&conn), 0, "{value}: stored as 0");
        tenx::insert(&conn, &Fx::subject("tenp-idle-subject", now, 20, 5 * DAY));
        tenp::insert_subjects(&conn, "tenp-idle-due-", 2, now - 60 * DAY, now - 40 * DAY, "self_offboard");
        let before = tenp::dump(&conn, &["prune_log", "blobs", "call_results"]);
        for at in [now, now + GRACE + DAY, now + 2 * GRACE] {
            let block = db.prune_once_at(at).await.expect("sweep").synthetic;
            assert_eq!((block["expire"].as_str(), block["purge"].as_str()), (Some("off"), Some("off")), "{value}: {block}");
            assert!(block.get("purge_due_unix").is_none(), "{value}: {block}");
        }
        tenp::assert_same_dump(&before, &tenp::dump(&conn, &["prune_log", "blobs", "call_results"]), "the sweeps changed a row");
        assert_eq!(tenp::count(&conn, "SELECT COUNT(*) FROM tenant_lifecycle_events"), 0);
    }
}

#[tokio::test]
async fn tenp_ac07_a_disabled_subjects_key_is_refused_and_moves_no_due_date_and_a_new_peer_never_gets_its_namespace() {
    let env = EnvGuard::clear(GRACE_ENV).await;
    let (server, conn, now) = server_with_floor().await;
    let db = &server.state.db;
    let (ns, key) = signup(&server.base_url, "Tenp Offboarder").await;
    // The harness's tenants carry both markers; make it explicit whatever the source class.
    conn.execute("UPDATE tenants SET origin = 'synthetic', synthetic = 'synthorg:test' WHERE namespace = ?1", [&ns]).unwrap();
    McpClient::with_bearer(&server.base_url, &key).tools_call("host.self_offboard", json!({})).await.expect("self_offboard");
    let seen = now - 5 * DAY;
    conn.execute("UPDATE tenants SET last_seen_unix = ?1 WHERE namespace = ?2", rusqlite::params![seen, ns]).unwrap();
    let id = id_of(&conn, &ns);
    let last_seen = |conn: &Connection| -> Option<i64> {
        conn.query_row("SELECT last_seen_unix FROM tenants WHERE id = ?1", [id], |r| r.get(0)).unwrap()
    };
    let admin = McpClient::with_bearer(&server.base_url, ADMIN_KEY);
    let due = async || {
        let r = extract_structured(&admin.tools_call("admin.prune_now", json!({"dry_run": true})).await.expect("dry run"));
        r["synthetic_implicit_tenants"]["purge_due_unix"].as_i64()
    };
    let due_before = due().await;
    assert_eq!(due_before, Some(seen.max(now - 30 * DAY) + GRACE), "dated from the last activity");

    // The old key as the tenant_key argument is refused as an unknown key; as a bearer, as disabled.
    let anonymous = McpClient::new(&server.base_url);
    let err = anonymous.tools_call("host.whoami", json!({"tenant_key": key})).await.expect_err("refused");
    assert_eq!(err.error_code.as_deref(), Some("tenant_key_invalid"));
    let err = McpClient::with_bearer(&server.base_url, &key).tools_call("host.whoami", json!({})).await.expect_err("refused");
    assert_eq!(err.error_code.as_deref(), Some("tenant_disabled"));
    assert_eq!(last_seen(&conn), Some(seen), "a refused call does not advance last_seen_unix");
    assert_eq!(due().await, due_before, "and moves no due date");

    // The purge: one sweep arms, the next deletes.
    db.prune_once_at(now).await.expect("arming sweep");
    let swept = db.prune_once_at(now + 10 * DAY).await.expect("deleting sweep").synthetic;
    assert_eq!(swept["purged"], 1, "{swept}");
    assert!(!exists(&conn, &ns));
    assert_eq!(tenp::count(&conn, &format!("SELECT COUNT(*) FROM namespace_tombstones WHERE namespace = '{ns}'")), 1);
    let err = anonymous.tools_call("host.whoami", json!({"tenant_key": key})).await.expect_err("still refused");
    assert_eq!(err.error_code.as_deref(), Some("tenant_key_invalid"));

    // A keyless new peer still mints, and the draw skips the purged namespace.
    drop(env);
    let _candidates = EnvGuard::set(TEST_NAMESPACE_CANDIDATES_ENV, &format!("{ns},tenp-free-ns")).await;
    let peer = McpClient::new(&server.base_url).with_session_continuity();
    let published = peer
        .tools_call(
            "host.tool_publish",
            json!({"name": "hello", "kind": "echo", "spec": {"schema": {"type": "object"}}}),
        )
        .await
        .expect("a keyless peer still mints an implicit tenant");
    let minted = extract_structured(&published)["onboarding"]["tenant"].as_str().expect("onboarding.tenant").to_string();
    assert_eq!(minted, "tenp-free-ns", "never the purged namespace");
    assert!(!exists(&conn, &ns));
}

#[tokio::test]
async fn tenp_ac07_namespace_draw_skips_reserved_and_live_candidates_and_gives_up_after_the_limit() {
    let first = EnvGuard::set(TEST_NAMESPACE_CANDIDATES_ENV, "tenp-tomb-1,tenp-live-1,tenp-free-1").await;
    let (server, conn, now) = server_with_floor().await;
    conn.execute(
        "INSERT INTO namespace_tombstones (namespace, purged_unix, action) VALUES ('tenp-tomb-1', ?1, 'tenant_delete')",
        [now],
    )
    .unwrap();
    tenx::insert(&conn, &Fx::subject("tenp-live-1", now, 1, 60));

    {
        // A reserved candidate, a live one, then a free one: minted on the third draw, no error.
        let (minted, _key) = signup(&server.base_url, "Tenp Third Draw").await;
        assert_eq!(minted, "tenp-free-1");
    }
    drop(first);
    {
        // Sixteen reserved candidates: table_row_conflict after the limit, and no tenants row.
        let all: Vec<String> = (0..mcphost::retention::NAMESPACE_DRAW_LIMIT).map(|i| format!("tenp-t16-{i}")).collect();
        for ns in &all {
            conn.execute(
                "INSERT INTO namespace_tombstones (namespace, purged_unix, action) VALUES (?1, ?2, 'tenant_delete')",
                rusqlite::params![ns, now],
            )
            .unwrap();
        }
        let _list = EnvGuard::set(TEST_NAMESPACE_CANDIDATES_ENV, &all.join(",")).await;
        let before = tenp::count(&conn, "SELECT COUNT(*) FROM tenants");
        let err = McpClient::new(&server.base_url)
            .tools_call("signup", json!({"name": "Tenp Sixteen Draws"}))
            .await
            .expect_err("the draws ran out");
        assert_eq!(err.error_code.as_deref(), Some("table_row_conflict"), "{}", err.message);
        assert_eq!(tenp::count(&conn, "SELECT COUNT(*) FROM tenants"), before, "no tenants row");
        for ns in &all {
            assert!(!exists(&conn, ns));
        }
    }
    // A direct create with a reserved namespace is refused by name.
    let err = server
        .state
        .db
        .create_tenant_attributed(
            "Tenp Direct".to_string(),
            "tenp-tomb-1".to_string(),
            "hash-direct".to_string(),
            None,
            None,
            None,
            None,
            "unattributed".to_string(),
            None,
        )
        .await
        .expect_err("reserved");
    match err {
        mcphost::errors::AppError::Structured { code, .. } => assert_eq!(code, "namespace_reserved"),
        other => panic!("expected namespace_reserved, got {other:?}"),
    }
    assert!(!exists(&conn, "tenp-tomb-1"));
}
