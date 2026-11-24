//! PRD-mcphost-synthetic-tenant-purge
//! AC7 (P0) — Given the § Classes rows, When sweeps pass the window and grace,
//! Then only the both-markers unclaimed row gains event, flag and deletion, the
//! other eight none, even if a class flips between mark and delete; grace env
//! off purges nothing, idle-days env off disables or purges nothing; a disabled
//! subject's old `tenant_key` gets `tenant_key_invalid` and moves no due date,
//! and a keyless new peer still mints, never a purged namespace.
//!
//! Also R6: the draw seam (`MCPHOST_TEST_ONLY_NAMESPACE_CANDIDATES`), the
//! tombstone on every delete path and the `namespace_reserved` refusal.

use crate::common::tenp::{self, DAY, Row};
use crate::common::{ADMIN_KEY, EnvGuard, McpClient, TestServer, extract_structured, signup};
use mcphost::db::Db;
use rusqlite::{Connection, params};
use serde_json::{Value, json};

const CANDIDATES_ENV: &str = "MCPHOST_TEST_ONLY_NAMESPACE_CANDIDATES";

/// The nine § Classes rows: (label, the row's fields). Only the first is a subject.
fn classes(prefix: &str, enabled: bool, last_seen: i64) -> Vec<(&'static str, Row)> {
    let base = |label: &str| Row {
        disabled: !enabled,
        reason: (!enabled).then_some("idle_expired"),
        created_unix: last_seen - 5 * DAY,
        ..Row::subject(&format!("{prefix}-{label}"), last_seen)
    };
    vec![
        ("both", base("both")),
        ("label_only", Row { origin: "external", ..base("label_only") }),
        ("origin_only", Row { synthetic: None, ..base("origin_only") }),
        ("neither", Row { origin: "external", synthetic: None, ..base("neither") }),
        ("claimed", Row { claimed_unix: Some(last_seen), ..base("claimed") }),
        ("paid", Row { paid_unix: Some(last_seen), ..base("paid") }),
        ("billing_ref", Row { billing_ref: Some("sub_tenp"), ..base("billing_ref") }),
        ("verified", Row { owner_verified_at: Some(last_seen), ..base("verified") }),
        (
            "admin_disable",
            Row { disabled: true, reason: Some("admin_disable"), ..base("admin_disable") },
        ),
    ]
}

fn events(conn: &Connection, id: i64) -> i64 {
    tenp::count(conn, &format!("SELECT COUNT(*) FROM tenant_lifecycle_events WHERE tenant_id = {id}"))
}

fn flag(conn: &Connection, id: i64) -> (bool, Option<String>) {
    crate::common::tenx::state(conn, id)
}

fn purge_audits(conn: &Connection) -> i64 {
    tenp::count(conn, &format!("SELECT COUNT(*) FROM admin_events WHERE action = '{}'", tenp::ACTION))
}

async fn seeded(floor: i64) -> (TestServer, Connection) {
    let server = TestServer::start().await;
    server.state.db.seed_retention_policy_from_env().await.expect("seed");
    let conn = tenp::open_raw(&server.data_dir.0);
    crate::common::tenx::set_floor(&conn, floor);
    tenp::arm_gate(&conn, floor);
    (server, conn)
}

#[tokio::test]
async fn tenp_ac07_only_the_both_markers_unclaimed_row_is_disabled_then_deleted() {
    let _env = EnvGuard::clear(tenp::GRACE_ENV).await;
    let now = mcphost::state::now_unix();
    let floor = now - 100 * DAY;
    let (server, conn) = seeded(floor).await;

    // Expire: enabled rows idle past the window (a day) but inside every other window.
    let enabled: Vec<(&str, i64)> = classes("tenp-e", true, now - 2 * DAY)
        .into_iter()
        .map(|(label, row)| (label, tenp::insert(&conn, &row)))
        .collect();
    let expired = server.state.db.prune_once_at(now).await.expect("sweep").synthetic;
    assert_eq!(expired["expired"], 1, "{expired}");
    for (label, id) in &enabled {
        if *label == "both" {
            assert_eq!(flag(&conn, *id), (true, Some("idle_expired".to_string())), "the subject is disabled");
            assert_eq!(events(&conn, *id), 1, "with one lifecycle event");
        } else if *label == "admin_disable" {
            assert_eq!(flag(&conn, *id), (true, Some("admin_disable".to_string())), "{label} keeps its reason");
            assert_eq!(events(&conn, *id), 0, "{label}");
        } else {
            assert_eq!(flag(&conn, *id), (false, None), "{label} is not disabled");
            assert_eq!(events(&conn, *id), 0, "{label} gains no event");
        }
    }

    // Purge: the same nine, disabled and past the grace.
    let disabled: Vec<(&str, i64)> = classes("tenp-p", false, floor - 5 * DAY)
        .into_iter()
        .map(|(label, row)| (label, tenp::insert(&conn, &row)))
        .collect();
    let purged = server.state.db.prune_once_at(now).await.expect("sweep").synthetic;
    assert_eq!(purged["purged"], 1, "{purged}");
    assert_eq!(purge_audits(&conn), 1);
    for (label, id) in &disabled {
        if *label == "both" {
            assert!(!tenp::tenant_exists(&conn, *id), "the subject is deleted");
        } else {
            assert!(tenp::tenant_exists(&conn, *id), "{label} survives the purge");
            assert_eq!(events(&conn, *id), 0, "{label} gains no event");
        }
    }
    let (_, subject) = enabled[0];
    assert!(tenp::tenant_exists(&conn, subject), "the expired subject is not yet past its own grace");
}

#[tokio::test]
async fn tenp_ac07_a_class_that_flips_between_mark_and_delete_wins() {
    let _env = EnvGuard::clear(tenp::GRACE_ENV).await;
    let now = mcphost::state::now_unix();
    let floor = now - 100 * DAY;
    let (server, conn) = seeded(floor).await;
    let dir = server.data_dir.0.clone();

    // Pairs (A, B): A is purged first; the cleanup that follows A's delete flips B, which the
    // batch already listed. B must survive, with whatever the flip wrote.
    let flips = [
        ("claim", "UPDATE tenants SET claimed_unix = 1 WHERE id = {B}"),
        ("verify", "UPDATE tenants SET owner_verified_at = 1 WHERE id = {B}"),
        ("pay", "UPDATE tenants SET paid_unix = 1 WHERE id = {B}"),
        ("billing_ref", "UPDATE tenants SET billing_ref = 'sub_flip' WHERE id = {B}"),
        ("enable", "UPDATE tenants SET disabled = 0, disabled_reason = NULL WHERE id = {B}"),
    ];
    let mut by_a = std::collections::HashMap::new();
    let mut survivors = Vec::new();
    for (label, sql) in flips {
        let a = tenp::insert(&conn, &Row { created_unix: floor - 6 * DAY, ..Row::subject(&format!("tenp-flip-a-{label}"), floor - 5 * DAY) });
        let b = tenp::insert(&conn, &Row { created_unix: floor - 6 * DAY, ..Row::subject(&format!("tenp-flip-b-{label}"), floor - 5 * DAY) });
        assert!(b > a);
        by_a.insert(a, sql.replace("{B}", &b.to_string()));
        survivors.push((label, b));
    }
    let by_a = std::sync::Arc::new(by_a);
    server.state.db.set_tenant_purge_cleanup(std::sync::Arc::new(move |purged: i64| {
        let by_a = by_a.clone();
        let dir = dir.clone();
        Box::pin(async move {
            if let Some(sql) = by_a.get(&purged) {
                tenp::open_raw(&dir).execute_batch(sql).expect("flip the class");
            }
        })
    }));

    let block = server.state.db.prune_once_at(now).await.expect("sweep").synthetic;
    assert_eq!(block["purged"], 5, "only the five A rows: {block}");
    assert_eq!(purge_audits(&conn), 5);
    for (label, b) in survivors {
        assert!(tenp::tenant_exists(&conn, b), "{label}: the class re-check inside the delete transaction wins");
    }
}

#[tokio::test]
async fn tenp_ac07_the_grace_env_off_purges_nothing_and_the_idle_env_off_disables_and_purges_nothing() {
    let now = mcphost::state::now_unix();
    let floor = now - 100 * DAY;

    {
        let _env = EnvGuard::set(tenp::GRACE_ENV, "off").await;
        let (server, conn) = seeded(floor).await;
        let due = tenp::insert(&conn, &Row { created_unix: floor - 6 * DAY, ..Row::subject("tenp-grace-off", floor - 5 * DAY) });
        let idle = tenp::insert(&conn, &Row { disabled: false, reason: None, created_unix: now - 9 * DAY, ..Row::subject("tenp-grace-idle", now - 3 * DAY) });
        let block = server.state.db.prune_once_at(now).await.expect("sweep").synthetic;
        assert_eq!(block["purge"], "off", "{block}");
        assert!(block.get("purge_due_unix").is_none() && block.get("purged").is_none(), "{block}");
        assert_eq!(block["expired"], 1, "the idle-days switch is still on: {block}");
        assert!(tenp::tenant_exists(&conn, due), "nothing purged");
        assert_eq!(flag(&conn, idle), (true, Some("idle_expired".to_string())), "disable still runs");
        assert_eq!(purge_audits(&conn), 0);
        let logged = tenp::latest_block(&conn);
        assert!(logged.get("purge_due_unix").is_none(), "such a row never arms the gate: {logged}");
    }

    for value in ["off", "0"] {
        let _env = EnvGuard::set(crate::common::tenx::ENV, value).await;
        let (server, conn) = seeded(floor).await;
        let due = tenp::insert(&conn, &Row { created_unix: floor - 6 * DAY, ..Row::subject("tenp-idle-off", floor - 5 * DAY) });
        let idle = tenp::insert(&conn, &Row { disabled: false, reason: None, created_unix: now - 9 * DAY, ..Row::subject("tenp-idle-live", now - 3 * DAY) });
        let block = server.state.db.prune_once_at(now).await.expect("sweep").synthetic;
        assert_eq!(block["expire"], "off", "{value}: {block}");
        assert_eq!(block["purge"], "off", "{value}: {block}");
        assert!(block.get("purge_due_unix").is_none(), "{value}: {block}");
        assert!(tenp::tenant_exists(&conn, due), "{value}: nothing purged");
        assert_eq!(flag(&conn, idle), (false, None), "{value}: nothing disabled");
        assert_eq!(events(&conn, idle), 0);
        assert_eq!(purge_audits(&conn), 0);
    }
}

#[tokio::test]
async fn tenp_ac07_an_old_tenant_key_is_refused_without_moving_the_due_date_and_never_resolves_after_the_purge() {
    let env = EnvGuard::clear(tenp::GRACE_ENV).await;
    let server = TestServer::start().await;
    server.state.db.seed_retention_policy_from_env().await.expect("seed");
    let conn = tenp::open_raw(&server.data_dir.0);
    let now = mcphost::state::now_unix();
    let floor = now - 100 * DAY;
    crate::common::tenx::set_floor(&conn, floor);
    tenp::arm_gate(&conn, floor);

    let (ns, key) = signup(&server.base_url, "Tenp Old Key").await;
    let id: i64 = conn.query_row("SELECT id FROM tenants WHERE namespace = ?1", params![ns], |r| r.get(0)).unwrap();
    let seen = floor - 5 * DAY;
    conn.execute(
        "UPDATE tenants SET disabled = 1, disabled_reason = 'idle_expired', signup_source = 'implicit', \
         last_seen_unix = ?2, created_unix = ?2 WHERE id = ?1",
        params![id, seen],
    )
    .unwrap();
    let due_before = |conn: &Connection| -> (i64, Option<i64>) {
        conn.query_row("SELECT last_seen_unix, (SELECT MIN(created_unix) FROM tenant_lifecycle_events WHERE tenant_id = ?1) FROM tenants WHERE id = ?1", params![id], |r| Ok((r.get(0)?, r.get(1)?))).unwrap()
    };
    let before = due_before(&conn);
    let anon = McpClient::new(&server.base_url);
    for _ in 0..2 {
        let err = anon.tools_call("host.whoami", json!({"tenant_key": key})).await.expect_err("refused");
        assert_eq!(err.error_code.as_deref(), Some("tenant_key_invalid"));
    }
    assert_eq!(due_before(&conn), before, "a refused call moves neither last_seen nor the event clock");
    let dry = async |admin: &McpClient| -> Value {
        extract_structured(&admin.tools_call("admin.prune_now", json!({"dry_run": true})).await.expect("dry run"))
            ["synthetic_implicit_tenants"]["purge_due_unix"]
            .clone()
    };
    let admin = McpClient::with_bearer(&server.base_url, ADMIN_KEY);
    assert_eq!(dry(&admin).await, json!(floor + 14 * DAY), "the due date is still the floor's");

    // Purge it: the key stays refused, now because no tenant holds it.
    let block = server.state.db.prune_once_at(now).await.expect("sweep").synthetic;
    assert_eq!(block["purged"], 1, "{block}");
    assert!(!tenp::tenant_exists(&conn, id));
    let err = anon.tools_call("host.whoami", json!({"tenant_key": key})).await.expect_err("refused");
    assert_eq!(err.error_code.as_deref(), Some("tenant_key_invalid"));
    assert!(server.state.db.find_tenant_by_namespace(ns.clone()).await.expect("lookup").is_none(), "the namespace is unknown");

    // A keyless new peer still mints a tenant, and never the purged namespace: the first
    // candidate of the draw is the purged one, which is reserved, so the second is minted.
    let fresh = "t_fe55ed01";
    drop(env); // one env guard at a time
    let _draw = EnvGuard::set(CANDIDATES_ENV, &format!("{ns},{fresh}")).await;
    let (minted, minted_key) = signup(&server.base_url, "Tenp Old Key").await;
    assert_eq!(minted, fresh, "the purged namespace was skipped");
    assert_ne!(minted_key, key);
    assert_eq!(tenp::count(&conn, &format!("SELECT COUNT(*) FROM tenants WHERE namespace = '{ns}'")), 0);
    let bare = McpClient::new(&server.base_url).with_session_continuity();
    let first = bare.tools_call("host.whoami", json!({})).await.expect("a keyless peer still mints");
    let minted_implicit = extract_structured(&first);
    assert!(!minted_implicit.to_string().contains(&ns), "never the purged namespace: {minted_implicit}");
}

fn tombstone(conn: &Connection, ns: &str) {
    conn.execute(
        "INSERT OR IGNORE INTO namespace_tombstones (namespace, purged_unix, action) VALUES (?1, 1, 'tenant_delete')",
        params![ns],
    )
    .unwrap();
}

#[tokio::test]
async fn tenp_ac07_the_draw_skips_a_reserved_and_a_live_namespace_and_mints_the_free_one() {
    let server = TestServer::start().await;
    let conn = tenp::open_raw(&server.data_dir.0);
    let (live_ns, _) = signup(&server.base_url, "Tenp Live").await;
    tombstone(&conn, "t_dead0001");
    let _draw = EnvGuard::set(CANDIDATES_ENV, &format!("t_dead0001,{live_ns},t_free0003")).await;
    let before = tenp::count(&conn, "SELECT COUNT(*) FROM tenants");
    let (minted, _) = signup(&server.base_url, "Tenp Third Draw").await;
    assert_eq!(minted, "t_free0003", "minted on the third draw with no error");
    assert_eq!(tenp::count(&conn, "SELECT COUNT(*) FROM tenants"), before + 1);
}

#[tokio::test]
async fn tenp_ac07_sixteen_reserved_draws_end_in_table_row_conflict_and_no_row() {
    let server = TestServer::start().await;
    let conn = tenp::open_raw(&server.data_dir.0);
    let names: Vec<String> = (0..mcphost::retention::NAMESPACE_DRAW_LIMIT).map(|i| format!("t_dead{i:04x}")).collect();
    for ns in &names {
        tombstone(&conn, ns);
    }
    // One more candidate that is free: draws stop at the limit, so it is never reached.
    let _draw = EnvGuard::set(CANDIDATES_ENV, &format!("{},t_free0016", names.join(","))).await;
    let before = tenp::count(&conn, "SELECT COUNT(*) FROM tenants");
    let err = McpClient::new(&server.base_url)
        .tools_call("signup", json!({"name": "Tenp Exhausted"}))
        .await
        .expect_err("every draw collided");
    assert_eq!(err.error_code.as_deref(), Some("table_row_conflict"), "{}", err.message);
    assert_eq!(tenp::count(&conn, "SELECT COUNT(*) FROM tenants"), before, "no tenants row");
    assert_eq!(tenp::count(&conn, "SELECT COUNT(*) FROM tenants WHERE namespace = 't_free0016'"), 0);
}

#[tokio::test]
async fn tenp_ac07_a_direct_create_with_a_reserved_namespace_is_namespace_reserved() {
    let dir = crate::common::TempDataDir::new();
    let db = Db::open(&dir.0).expect("open");
    tombstone(&tenp::open_raw(&dir.0), "t_dead0001");
    let err = db
        .create_tenant_attributed(
            "Direct".into(),
            "t_dead0001".into(),
            "hash-direct".into(),
            None,
            None,
            None,
            None,
            "unattributed".into(),
            None,
        )
        .await
        .expect_err("reserved");
    match err {
        mcphost::errors::AppError::Structured { code, data, .. } => {
            assert_eq!(code, "namespace_reserved");
            assert_eq!(data["table"], "tenants");
            assert_eq!(data["column"], "namespace");
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(tenp::count(&tenp::open_raw(&dir.0), "SELECT COUNT(*) FROM tenants WHERE namespace = 't_dead0001'"), 0);
}

#[tokio::test]
async fn tenp_ac07_every_delete_path_writes_the_tombstone() {
    let server = TestServer::start().await;
    let conn = tenp::open_raw(&server.data_dir.0);
    let now = mcphost::state::now_unix();
    crate::common::tenx::set_floor(&conn, now - 100 * DAY);
    tenp::arm_gate(&conn, now - 100 * DAY);
    let admin = McpClient::with_bearer(&server.base_url, ADMIN_KEY);

    let (by_id, _) = signup(&server.base_url, "Tenp Tomb Id").await;
    admin.tools_call("admin.tenant_delete", json!({"tenant": by_id})).await.expect("admin.tenant_delete");
    let (by_prefix, _) = signup(&server.base_url, "Tenp-Prefix-Tomb").await;
    admin
        .tools_call("admin.tenant_delete_by_prefix", json!({"prefix": "Tenp-Prefix-Tomb", "dry_run": false}))
        .await
        .expect("admin.tenant_delete_by_prefix");
    let (external, _) = signup(&server.base_url, "Tenp Tomb External").await;
    server
        .state
        .db
        .delete_tenant_as(external.clone(), mcphost::retention::PURGE_ACTION)
        .await
        .expect("external purge path")
        .expect("deleted");
    let subject = tenp::insert(&conn, &Row { created_unix: now - 100 * DAY - 6 * DAY, ..Row::subject("tenp-tomb-purge", now - 105 * DAY) });
    let purged = server.state.db.prune_once_at(now).await.expect("sweep").synthetic;
    assert_eq!(purged["purged"], 1, "{purged}");
    assert!(!tenp::tenant_exists(&conn, subject));

    let actions = |ns: &str| -> Option<String> {
        conn.query_row("SELECT action FROM namespace_tombstones WHERE namespace = ?1", params![ns], |r| r.get(0)).ok()
    };
    assert_eq!(actions(&by_id).as_deref(), Some("tenant_delete"));
    assert_eq!(actions(&by_prefix).as_deref(), Some("tenant_delete"));
    assert_eq!(actions(&external).as_deref(), Some(mcphost::retention::PURGE_ACTION));
    assert_eq!(actions("tenp-tomb-purge").as_deref(), Some(tenp::ACTION));
    let purged_at: i64 = conn
        .query_row("SELECT purged_unix FROM namespace_tombstones WHERE namespace = ?1", params![by_id], |r| r.get(0))
        .unwrap();
    assert!((purged_at - now).abs() < 300, "dated by the delete: {purged_at}");
}

#[tokio::test]
async fn tenp_ac07_an_invite_claim_redraws_past_a_reserved_namespace() {
    let server = TestServer::start().await;
    let conn = tenp::open_raw(&server.data_dir.0);
    let (a_ns, a_key) = signup(&server.base_url, "Tenp Inviter").await;
    let a_client = McpClient::with_bearer(&server.base_url, &a_key);
    a_client
        .tools_call(
            "host.tool_publish",
            json!({"name": "mytool", "kind": "echo", "spec": {"schema": {"type": "object", "properties": {}, "required": []}}}),
        )
        .await
        .expect("publish");
    let created = a_client.tools_call("host.invite.create", json!({"share": ["mytool"]})).await.expect("invite");
    let url = extract_structured(&created)["url"].as_str().unwrap().to_string();
    let path = url.trim_start_matches(&server.base_url).to_string();
    tombstone(&conn, "t_dead0002");
    let _draw = EnvGuard::set(CANDIDATES_ENV, "t_dead0002,t_free0002").await;
    let b = McpClient::new(&server.base_url).with_path(&path).with_session_continuity();
    b.tools_call("host.tool.call", json!({"name": format!("{a_ns}.mytool")})).await.expect("the claim succeeds");
    let minted: Vec<String> = conn
        .prepare("SELECT namespace FROM tenants WHERE invited_by_tenant_id = (SELECT id FROM tenants WHERE namespace = ?1)")
        .unwrap()
        .query_map(params![a_ns], |r| r.get(0))
        .unwrap()
        .map(|r| r.unwrap())
        .collect();
    assert_eq!(minted, vec!["t_free0002".to_string()], "the reserved draw was skipped");
    let uses: Value = json!(tenp::count(&conn, "SELECT uses FROM invites WHERE kind = 'standard' AND uses > 0"));
    assert_eq!(uses, json!(1), "the refused draw did not consume a slot");
}
