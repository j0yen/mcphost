//! PRD-mcphost-synthetic-tenant-purge
//! AC7 (P0) — Given the § Classes rows, When sweeps pass the window and grace, Then only the both-markers
//! unclaimed row gains event, flag and deletion, the other eight none, even if a class flips between
//! mark and delete; grace env off purges nothing, idle-days env off disables or purges nothing; a
//! disabled subject's old tenant_key gets tenant_key_invalid and moves no due date, and a keyless new
//! peer still mints, never a purged namespace.

use crate::common::tenp::{self, DAY, GRACE_ENV, IDLE_ENV, Row};
use crate::common::{ADMIN_KEY, EnvGuard, McpClient, TestServer, extract_structured, signup};
use mcphost::auth::NAMESPACE_CANDIDATES_ENV;
use rusqlite::types::Value as Sql;
use rusqlite::{Connection, params};
use serde_json::json;

const PURGE_ACTION: &str = "tenant_purge_synthetic";

/// A § Classes row, disabled the way the purge stage selects (`idle_expired`, 30 days idle) when
/// `enabled` is false, or enabled and idle 5 days (a subject of the disable stage) when true.
fn class_row(kind: &str, ns: &str, now: i64, enabled: bool) -> Row {
    let base = if enabled {
        Row::implicit(ns, now, 30, 5 * DAY)
    } else {
        Row::purge_subject(ns, now, "idle_expired", 30, 30)
    };
    match kind {
        "both" => base,
        "label_only" => Row { origin: "external", ..base },
        "origin_only" => Row { synthetic: None, ..base },
        // `unattributed`, not `external`: an unclaimed `external` row belongs to the external
        // policy's own stage, which this PRD leaves alone.
        "neither" => Row { origin: "unattributed", synthetic: None, ..base },
        "claimed" => Row { claimed_unix: Some(now - 10 * DAY), ..base },
        "paid" => Row { paid_unix: Some(now - 10 * DAY), ..base },
        "billing_ref" => Row { billing_ref: Some("cus_tenp7"), ..base },
        "owner_verified" => Row { owner_verified_at: Some(now - 10 * DAY), ..base },
        "admin_disable" => Row { disabled: true, disabled_reason: Some("admin_disable"), ..base },
        other => panic!("unknown class {other}"),
    }
}

/// The eight rows that are never touched.
const OTHER_CLASSES: [&str; 8] =
    ["label_only", "origin_only", "neither", "claimed", "paid", "billing_ref", "owner_verified", "admin_disable"];

/// Every column a stage may write, as text, so "none" means byte-identical.
fn fingerprint(conn: &Connection, id: i64) -> String {
    conn.query_row(
        "SELECT disabled, disabled_reason, origin, synthetic, signup_source, claimed_unix, owner_verified_at, \
         paid_unix, billing_ref, last_seen_unix, created_unix FROM tenants WHERE id = ?1",
        params![id],
        |r| (0..11).map(|i| r.get::<_, Sql>(i)).collect::<Result<Vec<_>, _>>(),
    )
    .map(|v| format!("{v:?}"))
    .unwrap_or_else(|_| "gone".to_string())
}

/// The synthetic policy's lifecycle events for one tenant. (A row with neither marker is an
/// unclaimed non-synthetic tenant, so the external policy's own warn stage may log for it; that
/// stage is not this PRD's and never flags or deletes.)
fn events(conn: &Connection, id: i64) -> i64 {
    tenp::count(conn, &format!("SELECT COUNT(*) FROM tenant_lifecycle_events WHERE tenant_id = ?1 AND policy = '{}'", tenp::POLICY), id)
}

fn tombstone(conn: &Connection, ns: &str) -> Option<String> {
    conn.query_row("SELECT action FROM namespace_tombstones WHERE namespace = ?1", params![ns], |r| r.get(0)).ok()
}

async fn sweep(server: &TestServer, at: i64) {
    server.state.db.prune_once_at(at).await.expect("sweep");
}

async fn started() -> (EnvGuard, TestServer, Connection, i64) {
    let guard = EnvGuard::clear(tenp::BUDGET_ENV).await;
    let server = TestServer::start().await;
    server.state.db.seed_retention_policy_from_env().await.expect("seed");
    let conn = Connection::open(server.data_dir.0.join("mcphost.db")).expect("raw db");
    let now = mcphost::state::now_unix();
    tenp::set_floor(&conn, now - 40 * DAY);
    (guard, server, conn, now)
}

#[tokio::test]
async fn tenp_ac07_only_the_both_markers_unclaimed_row_gains_event_flag_and_deletion() {
    let (_env, server, conn, t0) = started().await;

    // Purge stage: the nine classes already disabled idle_expired (the ninth by an admin).
    let mut disabled = Vec::new();
    for kind in ["both"].into_iter().chain(OTHER_CLASSES) {
        disabled.push((kind, tenp::insert(&conn, &class_row(kind, &format!("tenp7-d-{kind}"), t0, false))));
    }
    // Disable stage: the eight enabled classes (the ninth is disabled by definition), plus two
    // both-markers rows whose class flips after they are marked.
    let mut enabled = Vec::new();
    for kind in ["both"].into_iter().chain(OTHER_CLASSES.into_iter().filter(|k| *k != "admin_disable")) {
        enabled.push((kind, tenp::insert(&conn, &class_row(kind, &format!("tenp7-e-{kind}"), t0, true))));
    }
    let flip_claim = tenp::insert(&conn, &class_row("both", "tenp7-f-claim", t0, true));
    let flip_origin = tenp::insert(&conn, &class_row("both", "tenp7-f-origin", t0, true));
    let untouched: Vec<(String, i64, String)> = disabled
        .iter()
        .chain(enabled.iter())
        .filter(|(kind, _)| *kind != "both")
        .map(|(kind, id)| (kind.to_string(), *id, fingerprint(&conn, *id)))
        .collect();
    assert_eq!(untouched.len(), 8 + 7);
    let both_enabled = enabled[0].1;
    let both_disabled = disabled[0].1;

    // Sweep 1 finds no armed predecessor: it disables, and only reports the purge.
    sweep(&server, t0).await;
    let first = tenp::last_block(&conn);
    assert_eq!(first["purged"], 0, "{first}");
    assert_eq!(first["would_purge"], 1, "only the both-markers unclaimed row is a purge subject: {first}");
    assert!(tenp::exists(&conn, both_disabled));
    for id in [both_enabled, flip_claim, flip_origin] {
        assert_eq!(tenp::state(&conn, id), (true, Some("idle_expired".to_string())), "the window passed");
        assert_eq!(events(&conn, id), 1, "one lifecycle event with the flag");
    }
    sweep(&server, t0).await;
    assert!(!tenp::exists(&conn, both_disabled), "the second sweep deletes the due subject");
    assert_eq!(tombstone(&conn, "tenp7-d-both").as_deref(), Some(PURGE_ACTION));

    // The class flips between mark and delete: claimed after the event, and the origin marker lost.
    conn.execute("UPDATE tenants SET claimed_unix = ?1 WHERE id = ?2", params![t0, flip_claim]).unwrap();
    conn.execute("UPDATE tenants SET origin = 'external' WHERE id = ?1", params![flip_origin]).unwrap();
    let flipped = (fingerprint(&conn, flip_claim), fingerprint(&conn, flip_origin));

    // Grace elapses (the event is at t0), and again long after.
    let due = t0 + 14 * DAY + 60;
    sweep(&server, due).await;
    let block = tenp::last_block(&conn);
    assert_eq!(block["purged"], 1, "the unflipped marked row, nothing else: {block}");
    assert!(!tenp::exists(&conn, both_enabled), "the marked both-markers row is deleted after the grace");
    assert_eq!(tombstone(&conn, "tenp7-e-both").as_deref(), Some(PURGE_ACTION));
    sweep(&server, t0 + 400 * DAY).await;
    assert_eq!(tenp::last_block(&conn)["purged"], 0);

    assert_eq!((fingerprint(&conn, flip_claim), fingerprint(&conn, flip_origin)), flipped, "a flipped row is left as the flip made it");
    assert_eq!(events(&conn, flip_claim) + events(&conn, flip_origin), 2, "their events stay; no second event, no deletion");
    for (kind, id, before) in &untouched {
        assert_eq!(&fingerprint(&conn, *id), before, "{kind}: no flag change, no deletion");
        assert_eq!(events(&conn, *id), 0, "{kind}: no synthetic lifecycle event");
        assert!(tombstone(&conn, &format!("tenp7-d-{kind}")).is_none() && tombstone(&conn, &format!("tenp7-e-{kind}")).is_none(), "{kind}: no tombstone");
    }
    let deleted: i64 = conn.query_row("SELECT COUNT(*) FROM admin_events WHERE action = ?1", params![PURGE_ACTION], |r| r.get(0)).unwrap();
    assert_eq!(deleted, 2, "exactly the two both-markers unclaimed rows were purged");
}

#[tokio::test]
async fn tenp_ac07_grace_env_off_purges_nothing_and_removing_it_still_reports_once_first() {
    let off = EnvGuard::set(GRACE_ENV, "off").await;
    let server = TestServer::start().await;
    server.state.db.seed_retention_policy_from_env().await.expect("seed");
    let conn = Connection::open(server.data_dir.0.join("mcphost.db")).expect("raw db");
    let t0 = mcphost::state::now_unix();
    tenp::set_floor(&conn, t0 - 40 * DAY);
    let subject = tenp::insert(&conn, &class_row("both", "tenp7-grace", t0, false));

    for _ in 0..3 {
        sweep(&server, t0 + 400 * DAY).await;
        let block = tenp::last_block(&conn);
        assert_eq!(block["purge"], "off", "{block}");
        assert!(block.get("purge_due_unix").is_none(), "the arming token is never written with the grace off: {block}");
        assert!(tenp::exists(&conn, subject), "nothing is purged with the grace off");
    }
    drop(off);

    let _on = EnvGuard::clear(GRACE_ENV).await;
    sweep(&server, t0).await;
    let report = tenp::last_block(&conn);
    assert_eq!((report["purged"].as_i64(), report["would_purge"].as_i64()), (Some(0), Some(1)), "removing the env gives one report-only sweep: {report}");
    assert!(report.get("purge_due_unix").is_some() && report.get("purge").is_none(), "{report}");
    assert!(tenp::exists(&conn, subject));
    sweep(&server, t0).await;
    assert!(!tenp::exists(&conn, subject), "the next sweep deletes");
}

#[tokio::test]
async fn tenp_ac07_idle_days_env_off_or_zero_disables_and_purges_nothing() {
    let server = TestServer::start().await;
    let conn = Connection::open(server.data_dir.0.join("mcphost.db")).expect("raw db");
    let t0 = mcphost::state::now_unix();
    let mut ids = None;
    for value in ["off", "0"] {
        let _env = EnvGuard::set(IDLE_ENV, value).await;
        // The policy row takes the env when it is seeded, i.e. at each start of the host.
        server.state.db.seed_retention_policy_from_env().await.expect("seed");
        tenp::set_floor(&conn, t0 - 40 * DAY);
        let (idle, due) = *ids.get_or_insert_with(|| {
            (
                tenp::insert(&conn, &class_row("both", "tenp7-idle-live", t0, true)),
                tenp::insert(&conn, &class_row("both", "tenp7-idle-due", t0, false)),
            )
        });
        let c = &conn;
        for _ in 0..3 {
            sweep(&server, t0 + 400 * DAY).await;
            let block = tenp::last_block(c);
            assert_eq!(block["expire"], "off", "IDLE_DAYS={value}: {block}");
            assert_eq!(block["purge"], "off", "IDLE_DAYS={value}: {block}");
            assert!(block.get("purge_due_unix").is_none(), "{block}");
            assert_eq!(tenp::state(c, idle), (false, None), "IDLE_DAYS={value}: nothing is disabled");
            assert_eq!(events(c, idle), 0);
            assert!(tenp::exists(c, due), "IDLE_DAYS={value}: nothing is purged");
        }
    }

    // With the env gone and the policy seeded again the stages run: one report-only sweep, then the delete.
    let _clear = EnvGuard::clear(IDLE_ENV).await;
    server.state.db.seed_retention_policy_from_env().await.expect("seed");
    tenp::set_floor(&conn, t0 - 40 * DAY);
    let (c, (idle, due)) = (&conn, ids.unwrap());
    sweep(&server, t0).await;
    assert_eq!(tenp::state(c, idle), (true, Some("idle_expired".to_string())));
    assert!(tenp::exists(c, due));
    sweep(&server, t0).await;
    assert!(!tenp::exists(c, due));
}

#[tokio::test]
async fn tenp_ac07_a_disabled_subjects_old_key_is_refused_and_moves_no_due_date() {
    let (_env, server, conn, t0) = started().await;
    let (ns, key) = signup(&server.base_url, "AC7 abandoned").await;
    let id: i64 = conn.query_row("SELECT id FROM tenants WHERE namespace = ?1", params![ns], |r| r.get(0)).unwrap();
    let seen = t0 - 20 * DAY;
    conn.execute(
        "UPDATE tenants SET origin = 'synthetic', synthetic = 'synthorg:test', signup_source = 'implicit', disabled = 1, \
         disabled_reason = 'idle_expired', last_seen_unix = ?2, created_unix = ?3 WHERE id = ?1",
        params![id, seen, t0 - 60 * DAY],
    )
    .unwrap();
    let admin = McpClient::with_bearer(&server.base_url, ADMIN_KEY);
    let due_stamp = || async {
        let out = extract_structured(&admin.tools_call("admin.prune_now", json!({"dry_run": true})).await.expect("dry run"));
        out["synthetic_implicit_tenants"]["purge_due_unix"].clone()
    };
    let before = due_stamp().await;
    assert_eq!(before, seen + 14 * DAY, "the due stamp is last-seen plus the grace");

    let anon = McpClient::new(&server.base_url);
    let err = anon.tools_call("host.whoami", json!({"tenant_key": key})).await.expect_err("a disabled subject's key as tenant_key");
    assert_eq!(err.error_code.as_deref(), Some("tenant_key_invalid"));
    let err = McpClient::with_bearer(&server.base_url, &key).tools_call("host.whoami", json!({})).await.expect_err("as a header");
    assert_eq!(err.error_code.as_deref(), Some("tenant_disabled"), "the header path is unchanged");
    let last_seen: i64 = conn.query_row("SELECT last_seen_unix FROM tenants WHERE id = ?1", params![id], |r| r.get(0)).unwrap();
    assert_eq!(last_seen, seen, "a refused call never advances last_seen_unix");
    assert_eq!(due_stamp().await, before, "so the due date does not move");

    // The control: a live tenant's accepted call does advance it, so the check above is not vacuous.
    let (live_ns, live_key) = signup(&server.base_url, "AC7 control").await;
    McpClient::with_bearer(&server.base_url, &live_key).tools_call("host.whoami", json!({})).await.expect("live call");
    let live_seen: Option<i64> = conn.query_row("SELECT last_seen_unix FROM tenants WHERE namespace = ?1", params![live_ns], |r| r.get(0)).unwrap();
    assert!(live_seen.is_some_and(|s| s >= t0), "an accepted call stamps last_seen_unix");

    // After the purge the key resolves to nothing and is refused the same way.
    sweep(&server, t0).await;
    sweep(&server, t0).await;
    assert!(!tenp::exists(&conn, id), "purged");
    let err = anon.tools_call("host.whoami", json!({"tenant_key": key})).await.expect_err("a purged tenant's key");
    assert_eq!(err.error_code.as_deref(), Some("tenant_key_invalid"));

    // A keyless new peer still mints, and not under the purged namespace.
    let minted = extract_structured(&McpClient::new(&server.base_url).with_user_agent("tenp7-peer-mint").tools_call("host.whoami", json!({})).await.expect("keyless mint"));
    let new_ns = minted["onboarding"]["tenant"].as_str().expect("a new tenant").to_string();
    assert_ne!(new_ns, ns);
    assert!(tenp::exists(&conn, conn.query_row("SELECT id FROM tenants WHERE namespace = ?1", params![new_ns], |r| r.get(0)).unwrap()));
}

#[tokio::test]
async fn tenp_ac07_a_mint_redraws_past_a_reserved_or_taken_namespace_and_never_reissues_a_purged_one() {
    let env = EnvGuard::clear(NAMESPACE_CANDIDATES_ENV).await;
    let server = TestServer::start().await;
    server.state.db.seed_retention_policy_from_env().await.expect("seed");
    let conn = Connection::open(server.data_dir.0.join("mcphost.db")).expect("raw db");
    let t0 = mcphost::state::now_unix();
    tenp::set_floor(&conn, t0 - 40 * DAY);
    let tenants = |c: &Connection| -> i64 { c.query_row("SELECT COUNT(*) FROM tenants", [], |r| r.get(0)).unwrap() };
    let has = |c: &Connection, ns: &str| -> bool { c.query_row("SELECT COUNT(*) FROM tenants WHERE namespace = ?1", params![ns], |r| r.get::<_, i64>(0)).unwrap() == 1 };
    // Each keyless call is a new peer (the address plus the User-Agent), as the host remembers a
    // peer that just obtained a tenant for ten minutes.
    let peer = std::cell::Cell::new(0);
    let keyless = || async {
        peer.set(peer.get() + 1);
        McpClient::new(&server.base_url).with_user_agent(&format!("tenp7-peer-{}", peer.get())).tools_call("host.whoami", json!({})).await
    };

    // A purge writes the tombstone (the purge path of R6).
    tenp::insert(&conn, &class_row("both", "tenp7-purged", t0, false));
    sweep(&server, t0).await;
    sweep(&server, t0).await;
    assert_eq!(tombstone(&conn, "tenp7-purged").as_deref(), Some(PURGE_ACTION));

    // Tombstoned, then live, then free: the free one is minted on the third draw, no error.
    let (live_ns, _) = signup(&server.base_url, "AC7 live").await;
    conn.execute("INSERT INTO namespace_tombstones (namespace, purged_unix, action) VALUES ('t_dead0001', ?1, 'tenant_delete')", params![t0]).unwrap();
    env.replace(&format!("t_dead0001,{live_ns},t_fee00003"));
    let minted = extract_structured(&keyless().await.expect("the third draw is free"));
    assert_eq!(minted["onboarding"]["tenant"], "t_fee00003", "{minted}");
    assert!(has(&conn, "t_fee00003") && !has(&conn, "t_dead0001"));

    // A mint whose candidate equals a purged namespace gets a different one.
    env.replace("tenp7-purged,t_fee00004");
    let minted = extract_structured(&keyless().await.expect("redrawn"));
    assert_eq!(minted["onboarding"]["tenant"], "t_fee00004", "{minted}");
    assert!(!has(&conn, "tenp7-purged"), "the purged namespace is not re-issued");

    // Sixteen reserved candidates: `table_row_conflict` after NAMESPACE_DRAW_LIMIT draws, no tenants row.
    let sixteen: Vec<String> = (0..16).map(|i| format!("t_dead{:04}", 1000 + i)).collect();
    for ns in &sixteen {
        conn.execute("INSERT INTO namespace_tombstones (namespace, purged_unix, action) VALUES (?1, ?2, 'tenant_delete')", params![ns, t0]).unwrap();
    }
    let mut list: Vec<&str> = sixteen.iter().map(String::as_str).collect();
    list.push("t_fee00017");
    env.replace(&list.join(","));
    let before = tenants(&conn);
    let err = keyless().await.expect_err("sixteen draws all reserved");
    assert_eq!(err.error_code.as_deref(), Some("table_row_conflict"), "{err:?}");
    assert_eq!(tenants(&conn), before, "no tenants row");
    assert!(!has(&conn, "t_fee00017"), "the seventeenth candidate is never drawn");

    // A direct insert of a reserved namespace is `namespace_reserved`, in the table_row_conflict shape.
    let direct = server
        .state
        .db
        .create_tenant_attributed("direct".into(), "t_dead0001".into(), "hash-direct".into(), None, None, None, None, "external".into(), None)
        .await
        .expect_err("reserved");
    assert_eq!(direct.code(), "namespace_reserved");
    assert!(!has(&conn, "t_dead0001"));

    // Every delete path tombstones: admin.tenant_delete (AC3), the by-prefix delete here.
    for ns in ["tenp7del_a", "tenp7del_b"] {
        server.state.db.create_tenant_attributed(ns.into(), ns.into(), format!("hash-{ns}"), None, None, None, None, "external".into(), None).await.expect("plant");
    }
    let admin = McpClient::with_bearer(&server.base_url, ADMIN_KEY);
    admin.tools_call("admin.tenant_delete_by_prefix", json!({"prefix": "tenp7del_", "dry_run": false})).await.expect("by prefix");
    for ns in ["tenp7del_a", "tenp7del_b"] {
        assert!(!has(&conn, ns));
        assert_eq!(tombstone(&conn, ns).as_deref(), Some("tenant_delete"), "{ns}");
    }

    // The invite claim redraws too, and the slot taken by the refused attempt is not spent.
    let (_, a_key) = signup(&server.base_url, "AC7 inviter").await;
    let a = McpClient::with_bearer(&server.base_url, &a_key);
    let created = extract_structured(&a.tools_call("host.invite.create", json!({})).await.expect("invite"));
    let path = created["url"].as_str().unwrap().trim_start_matches(&server.base_url).to_string();
    env.replace("t_dead0001,t_fee00005");
    let first = extract_structured(
        &McpClient::new(&server.base_url).with_path(&path).with_session_continuity().tools_call("host.whoami", json!({})).await.expect("claim"),
    );
    assert!(first["onboarding"]["url"].is_string(), "{first}");
    assert!(has(&conn, "t_fee00005") && !has(&conn, "t_dead0001"));
    let listed = extract_structured(&a.tools_call("host.invite.list", json!({})).await.expect("list"));
    let claimed = listed["invites"].as_array().unwrap().iter().find(|i| i["kind"] == "standard").expect("the invite made above");
    assert_eq!((&claimed["uses"], &claimed["invitees"]), (&json!(1), &json!(["t_fee00005"])), "one claim, one use: {listed}");
}
