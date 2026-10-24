//! PRD-mcphost-external-unclaimed-tenant-expiry
//! AC7 (P0) — Given the email provider failing both attempts for A at idle day
//! 20, When the sweep runs on days 20, 21 and 30, Then day 20 writes
//! `warn_failed` with the provider status plus a `claim_email_events` row, day
//! 21 retries and writes `warned`, day 30 invalidates A because its first
//! attempt is 10 days old, and `prune_log.deleted_json` carries every stage
//! count and `pre_floor` under `external_unclaimed_tenants`.

use crate::common::texp::{self, DAY};
use crate::common::{EnvGuard, signup};
use mcphost::email::FakeEmailClient;
use mcphost::kinds::KindRegistry;
use serde_json::Value;
use std::sync::Arc;

/// Moves A's whole timeline `days` into the past, which is "the sweep runs `days` later".
fn age(conn: &rusqlite::Connection, id: i64, days: i64) {
    let secs = days * DAY;
    conn.execute("UPDATE tenants SET last_seen_unix = last_seen_unix - ?2 WHERE id = ?1", rusqlite::params![id, secs]).unwrap();
    conn.execute(
        "UPDATE tenant_lifecycle_events SET created_unix = created_unix - ?2, idle_since_unix = idle_since_unix - ?2 WHERE tenant_id = ?1",
        rusqlite::params![id, secs],
    )
    .unwrap();
}

fn rows(conn: &rusqlite::Connection) -> Vec<Value> {
    let mut stmt = conn.prepare("SELECT deleted_json FROM prune_log ORDER BY id").unwrap();
    stmt.query_map([], |r| r.get::<_, String>(0))
        .unwrap()
        .map(|j| serde_json::from_str::<Value>(&j.unwrap()).unwrap()["external_unclaimed_tenants"].clone())
        .collect()
}

#[tokio::test]
async fn texp_ac07_a_failed_warning_retries_the_next_day_and_never_delays_the_invalidation() {
    let _env = EnvGuard::clear(texp::ENV).await;
    let email = Arc::new(FakeEmailClient::new());
    let server = texp::server(email.clone(), KindRegistry::with_builtin()).await;
    server.state.db.seed_retention_policy_from_env().await.expect("seed");
    let conn = rusqlite::Connection::open(server.data_dir.0.join("mcphost.db")).expect("raw db");
    let now = mcphost::state::now_unix();
    texp::set_floor(&conn, now - 100 * DAY);
    let (ns, _key) = signup(&server.base_url, "Texp provider failure").await;
    let a = texp::make_external(&conn, &ns, now, 30, Some(20));
    texp::add_claim_attempt(&conn, a, "owner@example.com", now - 3 * DAY);

    // Day 20: both attempts fail with the provider's 503.
    email.fail_next_with_status(2, 503);
    server.state.db.prune_once().await.expect("day 20");
    assert_eq!(texp::stages(&conn, a), vec!["warn_failed"]);
    let status: Option<i64> = conn.query_row("SELECT status_code FROM tenant_lifecycle_events WHERE tenant_id = ?1", [a], |r| r.get(0)).unwrap();
    assert_eq!(status, Some(503), "the provider status rides on the event");
    let journal: (i64, Option<i64>) = conn
        .query_row("SELECT COUNT(*), MAX(status_code) FROM claim_email_events WHERE tenant_id = ?1", [a], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap();
    assert_eq!(journal, (1, Some(503)), "the existing claim_email_events journal row");
    assert_eq!(email.send_count(), 0);

    // Day 21: the retry goes through.
    age(&conn, a, 1);
    server.state.db.prune_once().await.expect("day 21");
    assert_eq!(texp::stages(&conn, a), vec!["warn_failed", "warned"]);
    assert_eq!(email.send_count(), 1);
    assert_eq!(texp::state(&conn, a), (false, None));

    // Day 30: the first attempt is 10 days old, so the key is invalidated and the notice goes out.
    age(&conn, a, 9);
    server.state.db.prune_once().await.expect("day 30");
    assert_eq!(texp::state(&conn, a), (true, Some("unclaimed_expired".to_string())));
    assert_eq!(texp::count_stage(&conn, a, "invalidated"), 1);
    assert_eq!(email.send_count(), 2, "the warning, then the notice");

    // Every sweep's prune_log row carries every stage count and pre_floor under the policy name.
    let blocks = rows(&conn);
    assert_eq!(blocks.len(), 3);
    for block in &blocks {
        for key in ["warned", "warn_no_address", "warn_failed", "invalidated", "notice_failed", "restored_since_last", "purged"] {
            assert!(block[key].is_i64(), "{key} missing from {block}");
        }
        assert!(block["pre_floor"]["count"].is_i64(), "pre_floor in {block}");
    }
    assert_eq!(blocks[0]["warn_failed"], 1);
    assert_eq!(blocks[1]["warned"], 1);
    assert_eq!(blocks[2]["invalidated"], 1);
}
