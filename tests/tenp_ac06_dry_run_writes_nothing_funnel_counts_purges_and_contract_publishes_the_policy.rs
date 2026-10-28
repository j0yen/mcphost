//! PRD-mcphost-synthetic-tenant-purge
//! AC6 (P0) — Given purge-due and pre-floor rows, When admin.prune_now is called with dry_run true,
//! Then it returns would_expire, pre_floor, would_purge, purge_due_unix, cross_refs by column and
//! class and held_entangled, writes no prune_log row and deletes nothing; admin.funnel carries
//! tenants_synthetic_purged; contract dump shows the § Contract keys and contract --check fails when
//! a constant moves.

use crate::common::tenp::{self, DAY, Planter, Row};
use crate::common::{ADMIN_KEY, EnvGuard, McpClient, TestServer, extract_structured};
use mcphost::api_contract::{CONTRACT_VERSION, ContractSnapshot, dump_contract, dump_contract_bytes, external_unclaimed, synthetic_purge, tenant_lifecycle};
use mcphost::kinds::KindRegistry;
use rusqlite::types::Value as Sql;
use rusqlite::Connection;
use serde_json::{Value, json};
use std::process::Command;

fn count(conn: &Connection, table: &str) -> i64 {
    conn.query_row(&format!("SELECT COUNT(*) FROM \"{table}\""), [], |r| r.get(0)).unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn tenp_ac06_dry_run_returns_the_sweeps_numbers_and_writes_and_deletes_nothing() {
    // Grace `off` (the prod state until a restore-verified copy exists): the dry run still computes.
    let _env = EnvGuard::set(tenp::GRACE_ENV, "off").await;
    let server = TestServer::start().await;
    server.state.db.seed_retention_policy_from_env().await.expect("seed");
    let conn = Connection::open(server.data_dir.0.join("mcphost.db")).expect("raw db");
    conn.execute_batch("PRAGMA foreign_keys = OFF").unwrap();
    let now = mcphost::state::now_unix();
    tenp::set_floor(&conn, now - 40 * DAY);
    let planter = Planter::new(&conn, now - 3600);

    // Two purge-due subjects: one clean, one named by a claimed tenant's call row.
    let clean = tenp::insert(&conn, &Row::purge_subject("tenp6-clean", now, "idle_expired", 60, 30));
    let held = tenp::insert(&conn, &Row::purge_subject("tenp6-held", now, "self_offboard", 60, 30));
    let claimed = tenp::insert(&conn, &Row { claimed_unix: Some(now - DAY), ..Row::implicit("tenp6-claimed", now, 5, 3600) });
    let synthetic = tenp::insert(&conn, &Row::implicit("tenp6-owner", now, 5, 3600));
    planter.plant_with(&conn, "calls", "tenant_id", claimed, 1, &[("caller_tenant_id", Sql::Integer(held))]).unwrap();
    planter.plant_with(&conn, "calls", "tenant_id", synthetic, 2, &[("caller_tenant_id", Sql::Integer(clean))]).unwrap();
    // Pre-floor and idle (would expire at the next sweep), pre-floor and recent, post-floor and recent.
    let idle = tenp::insert(&conn, &Row { last_seen_unix: None, ..Row::implicit("tenp6-idle", now, 50, 0) });
    let recent_pre_floor = tenp::insert(&conn, &Row::implicit("tenp6-recent-pre", now, 50, 3600));
    let _recent = tenp::insert(&conn, &Row::implicit("tenp6-recent", now, 5, 3600));

    let state = |conn: &Connection| {
        (
            count(conn, "prune_log"),
            count(conn, "tenants"),
            count(conn, "tenant_lifecycle_events"),
            count(conn, "admin_events"),
            count(conn, "calls"),
        )
    };
    let before = state(&conn);
    let admin = McpClient::with_bearer(&server.base_url, ADMIN_KEY);

    let out = extract_structured(&admin.tools_call("admin.prune_now", json!({"dry_run": true})).await.expect("dry run"));
    let keys: Vec<&str> = out.as_object().unwrap().keys().map(String::as_str).collect();
    assert_eq!(keys, ["synthetic_implicit_tenants"], "{out}");
    let block = &out["synthetic_implicit_tenants"];
    let block_keys: Vec<&str> = block.as_object().unwrap().keys().map(String::as_str).collect();
    assert_eq!(block_keys, ["cross_refs", "held_entangled", "pre_floor", "purge_due_unix", "would_expire", "would_purge"], "no other key: {block}");
    assert_eq!(block["would_purge"], 2, "{block}");
    assert_eq!(block["purge_due_unix"], now - 30 * DAY + 14 * DAY, "the earliest due stamp: {block}");
    assert_eq!(block["pre_floor"], 2, "the idle row and the recent row minted before the floor: {block}");
    assert_eq!(block["would_expire"], 3, "class rows not yet idle: the two recent ones and the synthetic owner: {block}");
    let call = &block["cross_refs"]["calls.caller_tenant_id"];
    assert_eq!(call["outcome"], "nulled", "{block}");
    assert_eq!(call["synthetic_implicit"], 1, "split by the other tenant's class: {block}");
    assert_eq!(call["other"], 1, "{block}");
    assert_eq!(block["held_entangled"], json!({"calls.caller_tenant_id": 1}), "{block}");
    assert_eq!(state(&conn), before, "no prune_log row, no delete, no event, no journal line");
    assert!(tenp::exists(&conn, clean) && tenp::exists(&conn, held));
    assert_eq!(tenp::state(&conn, idle), (false, None), "the idle pre-floor row is reported, not disabled");

    // A sweep running at the same moment does not make the dry run busy: it never takes the mutex.
    let db = server.state.db.clone();
    let sweeping = tokio::spawn(async move { db.prune_once().await });
    let during = extract_structured(&admin.tools_call("admin.prune_now", json!({"dry_run": true})).await.expect("dry run during a sweep"));
    assert!(during["synthetic_implicit_tenants"].get("busy").is_none(), "{during}");
    assert!(during["synthetic_implicit_tenants"]["would_purge"].is_i64(), "{during}");
    sweeping.await.unwrap().expect("the concurrent sweep");

    // The real sweep with the grace env `off` purges nothing and writes `purge: "off"`, never the token.
    let block = tenp::last_block(&conn);
    assert_eq!(block["purge"], "off", "{block}");
    assert!(block.get("purge_due_unix").is_none(), "{block}");
    assert!(tenp::exists(&conn, clean) && tenp::exists(&conn, held));
    assert_eq!(count(&conn, "prune_log"), before.0 + 1, "only the real sweep wrote a prune_log row");
    assert_eq!(tenp::state(&conn, idle), (true, Some("idle_expired".to_string())), "the real sweep disables the idle row");
    assert_eq!(tenp::state(&conn, recent_pre_floor), (false, None));
}

#[tokio::test]
async fn tenp_ac06_funnel_counts_synthetic_purges_from_the_journal() {
    let _env = EnvGuard::clear(tenp::GRACE_ENV).await;
    let server = TestServer::start().await;
    server.state.db.seed_retention_policy_from_env().await.expect("seed");
    let conn = Connection::open(server.data_dir.0.join("mcphost.db")).expect("raw db");
    let now = mcphost::state::now_unix();
    tenp::set_floor(&conn, now - 40 * DAY);
    let admin = McpClient::with_bearer(&server.base_url, ADMIN_KEY);
    let funnel = extract_structured(&admin.tools_call("admin.funnel", json!({})).await.expect("funnel"));
    assert_eq!(funnel["tenants_synthetic_purged"], 0, "{funnel}");
    assert_eq!(funnel["tenants_idle_expired"], 0, "beside the existing count: {funnel}");

    for i in 0..3 {
        tenp::insert(&conn, &Row::purge_subject(&format!("tenp6-f{i}"), now, "idle_expired", 60, 30));
    }
    server.state.db.prune_once().await.expect("report-only sweep");
    server.state.db.prune_once().await.expect("purging sweep");
    let funnel = extract_structured(&admin.tools_call("admin.funnel", json!({})).await.expect("funnel"));
    assert_eq!(funnel["tenants_synthetic_purged"], 3, "{funnel}");
    // A COUNT of the journal, not a counter: a fresh handle on the same file reads it too.
    let reopened = mcphost::db::Db::open(&server.data_dir.0).expect("reopen");
    assert_eq!(reopened.count_tenants_synthetic_purged().await.expect("count"), 3);
}

const COMMITTED: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/contracts/host-tools.v1.json");

fn check(path: &str, purge_grace_default: Option<&str>) -> std::process::Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_mcphost"));
    cmd.args(["contract", "dump", "--check", "--path", path]);
    if let Some(days) = purge_grace_default {
        cmd.env("MCPHOST_TEST_ONLY_TENANT_LIFECYCLE_PURGE_GRACE_DEFAULT_DAYS", days);
    }
    cmd.output().expect("run mcphost contract dump --check")
}

#[tokio::test]
async fn tenp_ac06_contract_carries_the_purge_policy_and_check_fails_when_a_constant_moves() {
    // The wall-budget override a fixture sets must not reach the published `budget_secs`; the child
    // below inherits it too, and `--check` still passes.
    let _env = EnvGuard::set(tenp::BUDGET_ENV, "3").await;
    let committed: Value = serde_json::from_slice(&std::fs::read(COMMITTED).unwrap()).unwrap();
    let rule = &committed["tenant_lifecycle"];
    assert_eq!(rule, &tenant_lifecycle(), "the committed key is the one api_contract builds");
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
    assert_eq!(rule["purge"], synthetic_purge(), "built from the constants, with the budget override unset in the publication");
    // Every key the sibling policies publish is still there, the external block byte for byte.
    for key in ["policy", "env_var", "default_days", "disabled_reason"] {
        assert!(rule.get(key).is_some(), "{key} is kept");
    }
    assert_eq!(rule["selector"]["origin"], "synthetic");
    assert_eq!(rule["selector"]["signup_source"], "implicit");
    assert_eq!(rule["disabled_reason"], "idle_expired");
    let external = json!({
        "disabled_reason": "unclaimed_expired",
        "idle_days": 30,
        "idle_days_env": "MCPHOST_RETENTION_EXTERNAL_UNCLAIMED_TENANT_IDLE_DAYS",
        "purge_grace_days": 14,
        "selector": {
            "billing_ref": null, "claimed": false, "idle_clock_starts_no_earlier_than_policy_floor": true,
            "origin": "external", "paid": false, "plan": "free", "stripe_customer_id": null, "synthetic": null
        },
        "stages": ["warned", "warn_no_address", "warn_failed", "invalidated", "notice_failed", "restored", "purged"],
        "warn_lead_days": 10
    });
    assert_eq!(rule["external_unclaimed"], external, "external_unclaimed is unchanged");
    assert_eq!(external_unclaimed(), external);
    assert_eq!(committed["contract_version"], CONTRACT_VERSION);

    // The committed file is exactly what the binary dumps, and a server serves the same bytes.
    let kinds = KindRegistry::with_builtin();
    assert_eq!(std::fs::read(COMMITTED).unwrap(), dump_contract_bytes(&kinds), "committed == dump");
    assert_eq!(dump_contract(&kinds)["tenant_lifecycle"], *rule);
    let server = TestServer::start().await;
    let body = reqwest::Client::new()
        .get(format!("{}/contract.json", server.base_url))
        .send()
        .await
        .expect("GET /contract.json")
        .bytes()
        .await
        .expect("body");
    let served: Value = serde_json::from_slice(&body).expect("json");
    assert_eq!(served["tenant_lifecycle"]["purge"], rule["purge"], "the live judge reads the same policy");
    assert_eq!(body.as_ref(), ContractSnapshot::builtin().served.as_slice(), "served bytes are the binary's");

    let clean = check(COMMITTED, None);
    assert!(clean.status.success(), "clean checkout passes: {}", String::from_utf8_lossy(&clean.stderr));
    let moved = check(COMMITTED, Some("7"));
    assert!(!moved.status.success(), "a moved purge constant must fail --check");
    let text = format!("{}{}", String::from_utf8_lossy(&moved.stdout), String::from_utf8_lossy(&moved.stderr));
    assert!(text.contains("stale"), "check says the file is stale: {text}");
}
