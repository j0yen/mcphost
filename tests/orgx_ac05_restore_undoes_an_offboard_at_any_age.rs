//! PRD-mcphost-org-export-and-offboard-v2
//! AC5 — Given an org offboarded at any age, When the synthetic and external
//! expire and purge stages run and the owner then calls `host.org.restore`,
//! Then no tenant, tool or state row was deleted, every member disabled by
//! that offboard is enabled with a NULL reason and their tools answer, and one
//! `tenant_audit` row `org.restore` exists per member.

use std::sync::Arc;

use crate::common::orgx::fixture_org;
use crate::common::tenp::{self, DAY, Row};
use crate::common::texp;
use crate::common::{EnvGuard, extract_structured};
use mcphost::control::sweep_external_unclaimed;
use mcphost::email::FakeEmailClient;
use mcphost::kinds::KindRegistry;
use rusqlite::{Connection, params};
use serde_json::json;

/// `(tenants, tools, state rows)` of the given tenant ids.
fn rows_of(conn: &Connection, ids: &[i64]) -> (i64, i64, i64) {
    let list = ids.iter().map(i64::to_string).collect::<Vec<_>>().join(",");
    let n = |table: &str| tenp::count(conn, &format!("SELECT COUNT(*) FROM {table} WHERE tenant_id IN ({list})"));
    (tenp::count(conn, &format!("SELECT COUNT(*) FROM tenants WHERE id IN ({list})")), n("tools"), n("tenant_state_kv"))
}

#[tokio::test]
async fn orgx_ac05() {
    let _env = EnvGuard::clear(texp::ENV).await;
    let server = texp::server(Arc::new(FakeEmailClient::new()), KindRegistry::with_builtin()).await;
    server.state.db.seed_retention_policy_from_env().await.expect("seed");
    let conn = tenp::open_raw(&server.data_dir.0);
    let now = mcphost::state::now_unix();
    let floor = now - 500 * DAY;
    texp::set_floor(&conn, floor);
    tenp::seed_pre_prd_row(&conn, floor);

    let fx = fixture_org(&server, "acme", 2, 2).await;
    for client in std::iter::once(&fx.owner).chain(fx.members.iter().map(|(_, c)| c)) {
        client.tools_call("host.state.set", json!({"key": "k", "value": 1})).await.expect("state.set");
    }
    let ids: Vec<i64> = fx.namespaces().iter().map(|ns| texp::id_of(&conn, ns)).collect();
    let before = rows_of(&conn, &ids);
    assert_eq!(before, (3, 6, 3));

    let out = extract_structured(&fx.owner.tools_call("host.org.offboard", json!({"confirm": "acme"})).await.expect("offboard"));
    assert_eq!(out["members_disabled"], 3, "{out}");

    // The offboard is old, and every member is shaped as the purge class of the
    // synthetic stages (two markers, unclaimed, unpaid) and idle for a year.
    for ns in fx.namespaces() {
        conn.execute(
            "UPDATE tenants SET origin = 'synthetic', synthetic = 'synthorg:test', signup_source = 'implicit', \
             claimed_unix = NULL, owner_verified_at = NULL, paid_unix = NULL, billing_ref = NULL, \
             created_unix = ?2, last_seen_unix = ?3 WHERE namespace = ?1",
            params![ns, now - 400 * DAY, now - 380 * DAY],
        )
        .expect("age member");
    }
    conn.execute("UPDATE orgs SET offboarded_unix = ?1", params![now - 380 * DAY]).expect("age offboard");

    // Control: a purge-class tenant disabled `idle_expired` that the same stage does delete.
    let control = tenp::insert(&conn, &Row::subject("orgx-control", now - 380 * DAY));
    server.state.db.prune_once_at(now).await.expect("sweep that arms the purge");
    server.state.db.prune_once_at(now).await.expect("sweep that purges");
    assert!(!tenp::tenant_exists(&conn, control), "the synthetic purge is live: it deleted the idle_expired control");

    // The same members as the external class, with an `unclaimed_expired` control the purge deletes.
    for ns in fx.namespaces() {
        texp::make_external(&conn, &ns, now, 90, Some(60));
    }
    let minted = texp::mint(&server).await;
    let ext_control = texp::make_external(&conn, &minted.ns, now, 90, Some(44));
    conn.execute("UPDATE tenants SET disabled = 1, disabled_reason = 'unclaimed_expired' WHERE id = ?1", [ext_control]).expect("expire");
    texp::add_event(&conn, ext_control, "warned", now - 44 * DAY, now - 34 * DAY);
    texp::add_event(&conn, ext_control, "invalidated", now - 44 * DAY, now - 14 * DAY);
    texp::add_claim_attempt(&conn, ext_control, "owner@example.com", now - 39 * DAY);
    for days in [0, 20, 40, 60] {
        sweep_external_unclaimed(&server.state, now + days * DAY).await.expect("external sweep");
    }
    assert!(!tenp::tenant_exists(&conn, ext_control), "the external purge is live: it deleted the unclaimed_expired control");

    // Nothing of the org went: same tenants, tools and state rows, still switched off by the offboard.
    assert_eq!(rows_of(&conn, &ids), before, "no tenant, tool or state row was deleted");
    for id in &ids {
        assert_eq!(texp::state(&conn, *id), (true, Some("org_offboard".to_string())), "tenant {id} is still org-offboarded");
    }

    // The owner's own (disabled) key restores the org, at any age.
    let restored = extract_structured(&fx.owner.tools_call("host.org.restore", json!({})).await.expect("restore"));
    assert_eq!(restored["org"], "acme");
    assert_eq!(restored["members_restored"], 3, "{restored}");
    for id in &ids {
        assert_eq!(texp::state(&conn, *id), (false, None), "tenant {id} is enabled with a NULL reason");
    }
    assert_eq!(tenp::count(&conn, "SELECT offboarded_unix IS NULL FROM orgs WHERE name = 'acme'"), 1);
    assert_eq!(rows_of(&conn, &ids), before, "restoring deletes and creates nothing");

    // Their tools answer again.
    let mut clients = vec![(fx.owner_ns.clone(), "own", &fx.owner)];
    for (i, (ns, c)) in fx.members.iter().enumerate() {
        clients.push((ns.clone(), if i == 0 { "m0" } else { "m1" }, c));
    }
    for (ns, prefix, client) in clients {
        for k in 0..2 {
            let tool = format!("{ns}.{prefix}_{k}");
            client.tools_call(&tool, json!({})).await.unwrap_or_else(|e| panic!("{tool} must answer again: {} {}", e.code, e.message));
        }
    }

    // One org.restore audit row per member; a second restore has nothing left to do.
    let audited = tenp::count(&conn, "SELECT COUNT(DISTINCT tenant_id) FROM tenant_audit WHERE action = 'org.restore'");
    assert_eq!(audited, 3);
    assert_eq!(tenp::count(&conn, "SELECT COUNT(*) FROM tenant_audit WHERE action = 'org.restore'"), 3);
    let again = extract_structured(&fx.owner.tools_call("host.org.restore", json!({})).await.expect("second restore"));
    assert_eq!(again["members_restored"], 0, "{again}");
}
