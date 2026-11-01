//! PRD-mcphost-external-unclaimed-tenant-expiry
//! AC5 (P0) — Given subjects invalidated 14, 13 and 20 days ago, the 20-day
//! one re-enabled by `admin.tenant_enable`, When the sweep runs, Then only the
//! 14-day row is deleted through `delete_tenant`'s body with an `admin_events`
//! row `tenant_purge_expired`, the 13-day row waits, the re-enabled row has
//! `last_seen_unix` reset and is neither purged nor re-invalidated, and
//! `admin.funnel` reports both `tenants_unclaimed_*` counts after a restart.

use crate::common::texp::{self, DAY};
use crate::common::{ADMIN_KEY, EnvGuard, McpClient, extract_structured};
use mcphost::control::sweep_external_unclaimed;
use mcphost::email::FakeEmailClient;
use mcphost::kinds::KindRegistry;
use serde_json::json;
use std::sync::Arc;

#[tokio::test]
async fn texp_ac05_only_the_fourteen_day_row_is_purged_and_a_reenabled_row_starts_over() {
    let _env = EnvGuard::clear(texp::ENV).await;
    let server = texp::server(Arc::new(FakeEmailClient::new()), KindRegistry::with_builtin()).await;
    server.state.db.seed_retention_policy_from_env().await.expect("seed");
    let conn = rusqlite::Connection::open(server.data_dir.0.join("mcphost.db")).expect("raw db");
    let now = mcphost::state::now_unix();
    texp::set_floor(&conn, now - 100 * DAY);
    let admin = McpClient::with_bearer(&server.base_url, ADMIN_KEY);

    let mut rows = Vec::new();
    for days in [14, 13, 20] {
        let m = texp::mint(&server).await;
        let id = texp::make_external(&conn, &m.ns, now, 90, Some(days + 30));
        conn.execute("UPDATE tenants SET disabled = 1, disabled_reason = 'unclaimed_expired' WHERE id = ?1", [id]).unwrap();
        texp::add_event(&conn, id, "warned", now - (days + 30) * DAY, now - (days + 20) * DAY);
        texp::add_event(&conn, id, "invalidated", now - (days + 30) * DAY, now - days * DAY);
        texp::add_claim_attempt(&conn, id, "owner@example.com", now - (days + 25) * DAY);
        rows.push((m.ns, id));
    }
    let [(purged_ns, purged_id), (waiting_ns, waiting_id), (enabled_ns, enabled_id)] = [rows[0].clone(), rows[1].clone(), rows[2].clone()];

    let before = mcphost::state::now_unix();
    admin.tools_call("admin.tenant_enable", json!({"tenant": enabled_ns})).await.expect("tenant_enable");
    let (last_seen, _) = texp::clock(&conn, enabled_id);
    assert!(last_seen.expect("last_seen reset") >= before, "enable starts a fresh idle run");
    assert_eq!(texp::state(&conn, enabled_id), (false, None));
    assert_eq!(texp::count_stage(&conn, enabled_id, "restored"), 1);

    let out = sweep_external_unclaimed(&server.state, now).await.expect("sweep");
    assert_eq!(out["purged"], 1, "{out}");
    assert_eq!(out["restored_since_last"], 1, "{out}");

    let exists = |id: i64| -> bool { conn.query_row("SELECT COUNT(*) FROM tenants WHERE id = ?1", [id], |r| r.get::<_, i64>(0)).unwrap() == 1 };
    assert!(!exists(purged_id), "the 14-day row is deleted");
    let left_over: i64 = conn.query_row("SELECT COUNT(*) FROM tenant_lifecycle_events WHERE tenant_id = ?1", [purged_id], |r| r.get(0)).unwrap();
    assert_eq!(left_over, 0, "its lifecycle events went with it");
    let audit: Vec<(String, String)> = conn
        .prepare("SELECT action, tenant FROM admin_events WHERE action = 'tenant_purge_expired'")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(audit, vec![("tenant_purge_expired".to_string(), purged_ns)]);
    assert_eq!(texp::state(&conn, waiting_id), (true, Some("unclaimed_expired".to_string())), "{waiting_ns} waits for day 14");
    assert!(exists(enabled_id));
    assert_eq!(texp::state(&conn, enabled_id), (false, None), "the re-enabled row is not re-invalidated");
    assert_eq!(texp::count_stage(&conn, enabled_id, "invalidated"), 1, "no second invalidation");

    let funnel = extract_structured(&admin.tools_call("admin.funnel", json!({})).await.expect("funnel"));
    assert_eq!(funnel["tenants_unclaimed_expired"], 1, "{funnel}");
    assert_eq!(funnel["tenants_unclaimed_purged"], 1, "{funnel}");
    // A fresh handle on the same file (a restart) reads the same numbers: COUNTs, not counters.
    let reopened = mcphost::db::Db::open(&server.data_dir.0).expect("reopen");
    assert_eq!(reopened.count_tenants_unclaimed_expired().await.expect("count"), 1);
    assert_eq!(reopened.count_tenants_unclaimed_purged().await.expect("count"), 1);
}
