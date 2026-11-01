//! PRD-mcphost-synthetic-tenant-purge
//! AC6 (P0) — Given purge-due and pre-floor rows, When admin.prune_now is
//! called with dry_run true, Then it returns would_expire, pre_floor,
//! would_purge, purge_due_unix, cross_refs by column and class and
//! held_entangled, writes no prune_log row and deletes nothing; admin.funnel
//! carries tenants_synthetic_purged; contract dump shows the § Contract keys and
//! contract --check fails when a constant moves.

use crate::common::tenp::{self, DAY, GRACE_ENV, Kind};
use crate::common::tenx::{self, Fx};
use crate::common::{ADMIN_KEY, EnvGuard, McpClient, TestServer, extract_structured};
use mcphost::api_contract::{dump_contract, dump_contract_bytes, tenant_lifecycle};
use mcphost::kinds::KindRegistry;
use rusqlite::Connection;
use serde_json::{Value, json};
use std::process::Command;

const SKIP: &[&str] = &["blobs", "call_results"];
const COMMITTED: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/contracts/host-tools.v1.json");

fn id_of(conn: &Connection, ns: &str) -> i64 {
    conn.query_row("SELECT id FROM tenants WHERE namespace = ?1", [ns], |r| r.get(0)).unwrap()
}

#[tokio::test]
async fn tenp_ac06_dry_run_returns_the_counts_and_writes_nothing_even_with_the_grace_env_off() {
    // The grace env `off`: a sweep would purge nothing, the dry run still counts.
    let _grace = EnvGuard::set(GRACE_ENV, "off").await;
    let server = TestServer::start().await;
    server.state.db.seed_retention_policy_from_env().await.expect("seed");
    let conn = Connection::open(server.data_dir.0.join("mcphost.db")).expect("raw db");
    conn.pragma_update(None, "foreign_keys", "OFF").expect("foreign_keys off");
    let now = mcphost::state::now_unix();
    let floor = now - 30 * DAY;
    tenp::set_floor(&conn, floor);
    tenp::widen_row_retention(&conn);

    // Purge-due subjects: one named by a synthetic tenant, one by a claimed test account, one by nobody.
    tenp::insert_subjects(&conn, "tenp-due-", 3, now - 60 * DAY, now - 40 * DAY, "self_offboard");
    let synthetic = tenp::insert_tenant(&conn, "tenp-cp-syn", Kind::SyntheticImplicit, now);
    let claimed = tenp::insert_tenant(&conn, "tenp-cp-claimed", Kind::Claimed, now);
    tenp::plant(&conn, "contacts", &[("tenant_id", synthetic.to_string()), ("contact_tenant_id", id_of(&conn, "tenp-due-1").to_string())]);
    tenp::plant(&conn, "contacts", &[("tenant_id", claimed.to_string()), ("contact_tenant_id", id_of(&conn, "tenp-due-2").to_string())]);
    // Pre-floor rows: two idle (disabled by the next sweep), one seen an hour ago (would_expire).
    for i in 0..2 {
        tenx::insert(&conn, &Fx::subject(&format!("tenp-pre-{i}"), now, 60, 40 * DAY));
    }
    tenx::insert(&conn, &Fx::subject("tenp-pre-seen", now, 60, 3600));

    let before = tenp::dump(&conn, SKIP);
    let logs = tenp::count(&conn, "SELECT COUNT(*) FROM prune_log");
    let admin = McpClient::with_bearer(&server.base_url, ADMIN_KEY);
    let result = extract_structured(&admin.tools_call("admin.prune_now", json!({"dry_run": true})).await.expect("dry run"));

    assert_eq!(result["dry_run"], true, "{result}");
    let block = &result["synthetic_implicit_tenants"];
    let mut keys: Vec<&str> = block.as_object().expect("a block").keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(keys, ["cross_refs", "held_entangled", "pre_floor", "purge_due_unix", "would_expire", "would_purge"], "no other key: {block}");
    // Three enabled implicit class rows were minted before the floor; one has not idled the window.
    assert_eq!(block["pre_floor"], 3, "{block}");
    assert_eq!(block["would_expire"], 2, "the seen-an-hour-ago row and the synthetic counterpart: {block}");
    assert_eq!(block["would_purge"], 2, "three due, one held by the claimed account: {block}");
    assert_eq!(block["purge_due_unix"], floor + 14 * DAY, "{block}");
    let contacts = &block["cross_refs"]["contacts.contact_tenant_id"];
    assert_eq!(contacts["outcome"], "cascade", "{block}");
    assert_eq!((contacts["synthetic_implicit"].as_i64(), contacts["other"].as_i64()), (Some(1), Some(1)), "{block}");
    assert_eq!(block["held_entangled"], json!({"contacts.contact_tenant_id": 1}), "{block}");

    tenp::assert_same_dump(&before, &tenp::dump(&conn, SKIP), "the dry run changed a row");
    assert_eq!(tenp::count(&conn, "SELECT COUNT(*) FROM prune_log"), logs, "no prune_log row");
    assert_eq!(tenp::count(&conn, "SELECT COUNT(*) FROM tenants WHERE namespace LIKE 'tenp-due-%'"), 3, "nothing deleted");

    // Without the flag the call is the sweep as before, plus the new block (here: the purge is off).
    let swept = extract_structured(&admin.tools_call("admin.prune_now", json!({})).await.expect("sweep"));
    assert!(swept.get("deleted").is_some() && swept["synthetic_implicit_tenants"]["purge"] == "off", "{swept}");
    assert!(swept.get("dry_run").is_none(), "{swept}");
    assert_eq!(tenp::count(&conn, "SELECT COUNT(*) FROM prune_log"), logs + 1);
}

#[tokio::test]
async fn tenp_ac06_funnel_counts_the_synthetic_purges() {
    let _grace = EnvGuard::clear(GRACE_ENV).await;
    let server = TestServer::start().await;
    let db = &server.state.db;
    db.seed_retention_policy_from_env().await.expect("seed");
    let conn = Connection::open(server.data_dir.0.join("mcphost.db")).expect("raw db");
    let now = mcphost::state::now_unix();
    tenp::set_floor(&conn, now - 30 * DAY);
    tenp::widen_row_retention(&conn);
    tenp::insert_subjects(&conn, "tenp-gone-", 2, now - 60 * DAY, now - 40 * DAY, "idle_expired");

    let admin = McpClient::with_bearer(&server.base_url, ADMIN_KEY);
    let funnel = async || extract_structured(&admin.tools_call("admin.funnel", json!({})).await.expect("funnel"));
    assert_eq!(funnel().await["tenants_synthetic_purged"], 0);
    db.prune_once_at(now).await.expect("arming sweep");
    assert_eq!(funnel().await["tenants_synthetic_purged"], 0, "the first sweep only reports");
    db.prune_once_at(now).await.expect("deleting sweep");
    let after = funnel().await;
    assert_eq!(after["tenants_synthetic_purged"], 2, "{after}");
    // A COUNT of journal rows: a fresh handle on the same file reads the same number.
    let reopened = mcphost::db::Db::open(&server.data_dir.0).expect("reopen");
    assert_eq!(reopened.count_tenants_synthetic_purged().await.expect("count"), 2);
}

fn check(override_env: Option<(&str, &str)>) -> std::process::Output {
    // nextest archives run on another host; env! bakes the build path (leg536 ac06 ENOENT)
    let bin = std::env::var_os("NEXTEST_BIN_EXE_mcphost")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from(env!("CARGO_BIN_EXE_mcphost")));
    let mut cmd = Command::new(bin);
    cmd.args(["contract", "dump", "--check", "--path", COMMITTED]);
    if let Some((k, v)) = override_env {
        cmd.env(k, v);
    }
    cmd.output().expect("run mcphost contract dump --check")
}

#[tokio::test]
async fn tenp_ac06_contract_dump_carries_the_purge_rule_and_check_fails_when_a_constant_moves() {
    // Holds the process-wide env lock so no sibling test's variable reaches the child.
    let _env = EnvGuard::clear(tenx::ENV).await;
    let committed: Value = serde_json::from_slice(&std::fs::read(COMMITTED).unwrap()).unwrap();
    let rule = &committed["tenant_lifecycle"];
    assert_eq!(rule, &tenant_lifecycle(), "the committed key is the one api_contract builds");
    let kinds = KindRegistry::with_builtin();
    assert_eq!(std::fs::read(COMMITTED).unwrap(), dump_contract_bytes(&kinds), "committed == dump");
    assert_eq!(dump_contract(&kinds)["tenant_lifecycle"], *rule);

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
    // Every key the sibling policies publish is still there, the external block byte for byte.
    for key in ["policy", "env_var", "default_days", "disabled_reason", "external_unclaimed"] {
        assert!(rule.get(key).is_some(), "{key} survives");
    }
    assert_eq!(rule["external_unclaimed"]["purge_grace_days"], 14);
    assert_eq!(rule["external_unclaimed"]["disabled_reason"], "unclaimed_expired");

    let clean = check(None);
    assert!(clean.status.success(), "clean checkout passes: {}", String::from_utf8_lossy(&clean.stderr));
    // The test-only wall budget is never published, so moving it leaves the contract alone.
    let budget = check(Some((tenp::BUDGET_ENV, "1")));
    assert!(budget.status.success(), "the budget override is not a contract constant: {}", String::from_utf8_lossy(&budget.stderr));
    let moved = check(Some(("MCPHOST_TEST_ONLY_TENANT_LIFECYCLE_PURGE_GRACE_DAYS", "30")));
    assert!(!moved.status.success(), "a moved grace must fail --check");
    let text = format!("{}{}", String::from_utf8_lossy(&moved.stdout), String::from_utf8_lossy(&moved.stderr));
    assert!(text.contains("stale"), "check says the file is stale: {text}");
}
