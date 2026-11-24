//! PRD-mcphost-synthetic-tenant-purge
//! AC6 (P0) — Given purge-due and pre-floor rows, When `admin.prune_now` is
//! called with `dry_run` true, Then it returns `would_expire`, `pre_floor`,
//! `would_purge`, `purge_due_unix`, `cross_refs` by column and class and
//! `held_entangled`, writes no `prune_log` row and deletes nothing;
//! `admin.funnel` carries `tenants_synthetic_purged`; `contract dump` shows the
//! § Contract keys and `contract --check` fails when a constant moves.

use crate::common::tenp::{self, DAY, Row};
use crate::common::{ADMIN_KEY, EnvGuard, McpClient, TestServer, extract_structured};
use serde_json::{Value, json};
use std::process::Command;

const COMMITTED: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/contracts/host-tools.v1.json");

fn live(ns: &str, created_unix: i64, last_seen_unix: i64) -> Row {
    Row { disabled: false, reason: None, created_unix, last_seen_unix: Some(last_seen_unix), ..Row::subject(ns, last_seen_unix) }
}

fn disabled(ns: &str, last_seen_unix: i64) -> Row {
    Row { created_unix: last_seen_unix - DAY, ..Row::subject(ns, last_seen_unix) }
}

/// Plants one `calls` row owned by `owner` naming `subject` as its caller.
fn caller_row(conn: &rusqlite::Connection, owner: i64, subject: i64) {
    tenp::set_ids(conn, &[(subject, owner)]);
    tenp::plant(conn, "calls", &[("tenant_id", "other"), ("caller_tenant_id", "id")]);
}

fn table_counts(conn: &rusqlite::Connection) -> (i64, i64, i64, i64) {
    (
        tenp::count(conn, "SELECT COUNT(*) FROM tenants"),
        tenp::count(conn, "SELECT COUNT(*) FROM prune_log"),
        tenp::count(conn, "SELECT COUNT(*) FROM tenant_lifecycle_events"),
        tenp::count(conn, "SELECT COUNT(*) FROM admin_events"),
    )
}

#[tokio::test]
async fn tenp_ac06_dry_run_reports_the_numbers_and_writes_and_deletes_nothing() {
    // The grace env is off, as prod runs it at the deploy: the report still has numbers.
    let _env = EnvGuard::set(tenp::GRACE_ENV, "off").await;
    let server = TestServer::start().await;
    server.state.db.seed_retention_policy_from_env().await.expect("seed");
    let conn = tenp::open_raw(&server.data_dir.0);
    let now = mcphost::state::now_unix();
    let floor = now - 100 * DAY;
    crate::common::tenx::set_floor(&conn, floor);

    // Pre-floor, enabled, seen an hour ago: not idle yet (would_expire) and before the floor (pre_floor).
    live_pair(&conn, floor, now);
    // Enabled, idle three days: a sweep would disable it, so it is not "would_expire".
    tenp::insert(&conn, &live("tenp-idle", floor - 10 * DAY, now - 3 * DAY));
    // Due at floor + 14 days: three subjects, one of them named by a claimed tenant's row.
    let d1 = tenp::insert(&conn, &disabled("tenp-d1", floor - 5 * DAY));
    let d2 = tenp::insert(&conn, &disabled("tenp-d2", floor - 4 * DAY));
    let held = tenp::insert(&conn, &disabled("tenp-held", floor - 3 * DAY));
    // Not due: seen yesterday.
    tenp::insert(&conn, &disabled("tenp-fresh", now - DAY));
    let synthetic_owner = tenp::insert(&conn, &live("tenp-owner", now - DAY, now - 60));
    let claimed_owner = tenp::insert(&conn, &Row { disabled: false, reason: None, ..Row::claimed("tenp-claimed", now - 60) });
    caller_row(&conn, synthetic_owner, d1);
    caller_row(&conn, claimed_owner, held);

    let before = table_counts(&conn);
    let admin = McpClient::with_bearer(&server.base_url, ADMIN_KEY);
    let reply = extract_structured(&admin.tools_call("admin.prune_now", json!({"dry_run": true})).await.expect("dry run"));
    let block = &reply["synthetic_implicit_tenants"];
    assert_eq!(reply.as_object().unwrap().keys().collect::<Vec<_>>(), ["synthetic_implicit_tenants"], "{reply}");
    let mut keys: Vec<&str> = block.as_object().expect("object").keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(keys, ["cross_refs", "held_entangled", "pre_floor", "purge_due_unix", "would_expire", "would_purge"], "no other key: {block}");

    assert_eq!(block["would_expire"], 3, "the two pre-floor rows seen an hour ago and the synthetic owner; the claimed owner is no subject: {block}");
    assert_eq!(block["pre_floor"], 3, "the idle row is pre-floor too: {block}");
    assert_eq!(block["would_purge"], 2, "three due, one held: {block}");
    assert_eq!(block["purge_due_unix"], floor + 14 * DAY, "the earliest due stamp, clamped up to the floor: {block}");
    let calls = &block["cross_refs"]["calls.caller_tenant_id"];
    assert_eq!(calls["outcome"], "nulled", "{block}");
    assert_eq!(calls["synthetic_implicit"], 1, "{block}");
    assert_eq!(calls["other"], 1, "the claimed tenant's row: {block}");
    assert_eq!(block["held_entangled"]["calls.caller_tenant_id"], 1, "{block}");

    assert_eq!(table_counts(&conn), before, "no prune_log row, no event, no audit row, no delete");
    for id in [d1, d2, held] {
        assert!(tenp::tenant_exists(&conn, id), "nothing deleted");
    }
    assert_eq!(tenp::rows_naming(&conn, "calls", "caller_tenant_id", d1), 1, "no row nulled");

    // Never takes the sweep mutex: the method does not name it.
    let source = include_str!("../src/db/synthetic_purge.rs");
    let body = source.split("pub(crate) async fn synthetic_dry_run").nth(1).and_then(|s| s.split("\n    }\n").next()).expect("dry run body");
    assert!(!body.contains("sweep_lock") && !body.contains("INSERT") && !body.contains("DELETE") && !body.contains("UPDATE"), "{body}");
    assert!(body.contains("TxBehavior::Deferred"), "one read transaction");
}

fn live_pair(conn: &rusqlite::Connection, floor: i64, now: i64) {
    tenp::insert(conn, &live("tenp-pre-a", floor - 10 * DAY, now - 3600));
    tenp::insert(conn, &live("tenp-pre-b", floor - 10 * DAY, now - 7200));
}

#[tokio::test]
async fn tenp_ac06_funnel_counts_the_purged_synthetic_tenants() {
    let server = TestServer::start().await;
    server.state.db.seed_retention_policy_from_env().await.expect("seed");
    let conn = tenp::open_raw(&server.data_dir.0);
    let now = mcphost::state::now_unix();
    let floor = now - 100 * DAY;
    crate::common::tenx::set_floor(&conn, floor);
    tenp::arm_gate(&conn, floor);
    for i in 0..3 {
        tenp::insert(&conn, &disabled(&format!("tenp-f{i}"), floor - 5 * DAY));
    }
    let admin = McpClient::with_bearer(&server.base_url, ADMIN_KEY);
    let funnel = async |admin: &McpClient| -> Value {
        extract_structured(&admin.tools_call("admin.funnel", json!({})).await.expect("admin.funnel"))
    };
    let before = funnel(&admin).await;
    assert_eq!(before["tenants_synthetic_purged"], 0, "{before}");
    let sweep = server.state.db.prune_once_at(now).await.expect("sweep").synthetic;
    assert_eq!(sweep["purged"], 3, "{sweep}");
    let after = funnel(&admin).await;
    assert_eq!(after["tenants_synthetic_purged"], 3, "{after}");
    assert_eq!(after["tenants_unclaimed_purged"], before["tenants_unclaimed_purged"], "the external count is its own");
    assert!(after.get("tenants_idle_expired").is_some(), "beside tenants_idle_expired");
}

fn check(override_days: Option<&str>, budget: Option<&str>) -> std::process::Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_mcphost"));
    cmd.args(["contract", "dump", "--check", "--path", COMMITTED]);
    if let Some(days) = override_days {
        cmd.env("MCPHOST_TEST_ONLY_TENANT_LIFECYCLE_DEFAULT_DAYS", days);
    }
    if let Some(secs) = budget {
        cmd.env(tenp::BUDGET_ENV, secs);
    }
    cmd.output().expect("run mcphost contract dump --check")
}

#[tokio::test]
async fn tenp_ac06_contract_carries_the_purge_keys_and_check_fails_when_a_constant_moves() {
    let _env = EnvGuard::clear(tenp::GRACE_ENV).await;
    let committed: Value = serde_json::from_slice(&std::fs::read(COMMITTED).unwrap()).unwrap();
    let rule = &committed["tenant_lifecycle"];
    assert_eq!(rule, &mcphost::api_contract::tenant_lifecycle(), "the committed key is the one api_contract builds");
    assert_eq!(rule["zero_disables"], false);
    assert_eq!(rule["floor_rule"], "idle_from_floor");
    assert_eq!(rule["selector"]["minted_at_or_after_policy_floor"], false);
    assert_eq!(rule["selector"]["owner_verified"], false);
    assert_eq!(
        rule["purge"],
        json!({
            "grace_days": 14,
            "grace_env": "MCPHOST_RETENTION_SYNTHETIC_PURGE_GRACE_DAYS",
            "batch": 500,
            "budget_secs": 120,
            "vacuum_pages": 2048,
            "disabled_reasons": ["idle_expired", "self_offboard"],
            "signup_source": "any",
            "admin_event": "tenant_purge_synthetic",
            "first_sweep_reports_only": true,
        })
    );
    // Every key it had stays, and the external block is the one api_contract builds.
    for key in ["policy", "selector", "env_var", "default_days", "disabled_reason", "external_unclaimed"] {
        assert!(rule.get(key).is_some(), "{key} kept");
    }
    assert_eq!(rule["external_unclaimed"], mcphost::api_contract::external_unclaimed());
    assert_eq!(rule["policy"], "synthetic_implicit_tenants");
    assert_eq!(rule["selector"]["signup_source"], "implicit");

    let clean = check(None, None);
    assert!(clean.status.success(), "clean checkout passes: {}", String::from_utf8_lossy(&clean.stderr));
    // The test-only wall budget is never published: moving it leaves the dump alone.
    let budgeted = check(None, Some("0"));
    assert!(budgeted.status.success(), "the override is not in the contract: {}", String::from_utf8_lossy(&budgeted.stderr));
    let moved = check(Some("7"), None);
    assert!(!moved.status.success(), "a moved constant must fail --check");
    let text = format!("{}{}", String::from_utf8_lossy(&moved.stdout), String::from_utf8_lossy(&moved.stderr));
    assert!(text.contains("stale"), "check says the file is stale: {text}");
}
