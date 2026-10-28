//! solo-suite: 330 one test builds a 3.2 M-row fixture (about 60 s) and purges 6,000 subjects through paced transactions under a live call loop (about 250 s)
//! PRD-mcphost-synthetic-tenant-purge
//! AC1 (P0) — Given the § Fixture (6,000 due subjects, 20 heavy) and a host.tool_call loop on a live
//! tenant, When the scheduled sweep and admin.prune_now start together with a budget one sweep
//! cannot finish, Then one reports busy, every subject is deleted exactly once over two or more
//! batches, held_over equals the remainder, loop p95 stays under 250 ms and under max(2× the idle
//! baseline, 50 ms), and per-batch wall time in the PR body.

use crate::common::tenp::{self, DAY, Planter, Row};
use crate::common::{EnvGuard, McpClient, TestServer, extract_structured, perf_skipped, signup};
use serde_json::{Value, json};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

const SUBJECTS: usize = 6_000;
const HEAVY: usize = 20;
/// Prod's per-subject ratio (`seed1-prod-counts.txt` STEP 7e).
const WEIGHT: [(&str, i64); 5] =
    [("runs", 120), ("logs", 110), ("tenant_state_kv", 100), ("trigger_receipts", 37), ("host_tool_usage", 8)];
const HEAVY_KV: i64 = 50_000;

fn p95_ms(samples: &mut [u64]) -> u64 {
    samples.sort_unstable();
    samples.get((samples.len() * 95 / 100).min(samples.len().saturating_sub(1))).copied().unwrap_or(0)
}

/// Latency of `host.tool_call` on a live tenant until `stop`, in ms, sampled by one caller.
async fn loop_calls(base_url: String, key: String, tool: String, stop: Arc<AtomicBool>) -> Vec<u64> {
    let client = McpClient::with_bearer(&base_url, &key);
    let mut ms = Vec::new();
    while !stop.load(Ordering::Relaxed) {
        let t = Instant::now();
        client.tools_call(&tool, json!({})).await.expect("host.tool_call during the purge");
        ms.push(t.elapsed().as_millis() as u64);
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    ms
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn tenp_ac01_six_thousand_subjects_are_purged_exactly_once_while_calls_flow() {
    // One `EnvGuard` per test (they share one process-wide lock); the grace and idle envs are unset.
    // A budget one sweep cannot finish: each sweep stops after about this many seconds of purging.
    let _budget = EnvGuard::set(tenp::BUDGET_ENV, "20").await;
    // The timed tenant calls for minutes: lift the free plan's daily call ceiling.
    let server = TestServer::start_with_plans(|catalog| {
        for p in &mut catalog.plans {
            p.calls_per_day = i64::MAX / 4;
        }
    })
    .await;
    server.state.db.seed_retention_policy_from_env().await.expect("seed");
    let mut conn = rusqlite::Connection::open(server.data_dir.0.join("mcphost.db")).expect("raw db");
    conn.execute_batch("PRAGMA foreign_keys = OFF; PRAGMA synchronous = OFF").unwrap();
    let now = mcphost::state::now_unix();
    tenp::set_floor(&conn, now - 40 * DAY);

    let t0 = Instant::now();
    let step = |what: &str| eprintln!("tenp_ac01 [{:>6} ms] {what}", t0.elapsed().as_millis());
    // The live tenant whose calls are timed.
    let (_live_ns, live_key) = signup(&server.base_url, "AC1 live tenant").await;
    let live = McpClient::with_bearer(&server.base_url, &live_key);
    let published = live
        .tools_call("host.tool_publish", json!({"name": "pingpong", "kind": "echo", "spec": {"schema": {"type": "object"}}}))
        .await
        .expect("publish echo tool");
    let tool = extract_structured(&published)["name"].as_str().expect("tool name").to_string();

    step("live tenant ready");
    // The fixture: 6,000 disabled two-marker subjects, all due (disabled 30 days ago, floor 40 days ago).
    conn.set_prepared_statement_cache_capacity(512);
    let planter = Planter::new(&conn, now - 3600);
    let weights: Vec<_> = WEIGHT.iter().map(|(t, n)| (*t, *n)).collect();
    // Columns that name a subject from another tenant's row (two-sided) are AC2's: here every
    // planted row is the subject's own, so no subject is held.
    let two_sided: Vec<String> = mcphost::db::CROSS_REFS.iter().map(|r| r.key.to_string()).collect();
    let mut ids = Vec::with_capacity(SUBJECTS);
    let tx = conn.transaction().unwrap();
    for i in 0..SUBJECTS {
        let reason = if i % 2 == 0 { "idle_expired" } else { "self_offboard" };
        let id = tenp::insert(&tx, &Row::purge_subject(&format!("tenp-s{i}"), now, reason, 60, 30));
        for (n, (table, column)) in planter.keys().into_iter().enumerate() {
            if (weights.iter().any(|(t, _)| *t == table) && column == "tenant_id")
                || table == "tenant_lifecycle_events"
                || two_sided.contains(&format!("{table}.{column}"))
            {
                continue; // weighted below; an event would move the due date; two-sided is AC2's
            }
            planter.plant(&tx, &table, &column, id, n as i64).unwrap_or_else(|e| panic!("plant {table}.{column}: {e}"));
        }
        for (table, rows) in &weights {
            let rows = if *table == "tenant_state_kv" && i < HEAVY { HEAVY_KV } else { *rows };
            for k in 0..rows {
                planter.plant(&tx, table, "tenant_id", id, k).unwrap_or_else(|e| panic!("plant {table}: {e}"));
            }
        }
        ids.push(id);
    }
    tenp::seed_pre_prd_log_row(&tx, now - 3600);
    tx.commit().unwrap();
    step("fixture committed");
    let total_rows: i64 = weights.iter().map(|(t, _)| tenp_count_all(&conn, t)).sum();
    eprintln!("fixture: {SUBJECTS} subjects, {HEAVY} heavy, {total_rows} weight rows");

    step("counted");
    // Baseline: 10 s of calls with nothing else running.
    let stop = Arc::new(AtomicBool::new(false));
    let idle_loop = tokio::spawn(loop_calls(server.base_url.clone(), live_key.clone(), tool.clone(), stop.clone()));
    tokio::time::sleep(Duration::from_secs(10)).await;
    stop.store(true, Ordering::Relaxed);
    let mut baseline = idle_loop.await.unwrap();
    let baseline_p95 = p95_ms(&mut baseline);

    step("baseline done");
    // The first armed sweep carries no `purge_due_unix` ahead of it: it reports and deletes nothing.
    server.state.db.prune_once().await.expect("report-only sweep");
    let first = tenp::last_block(&conn);
    assert_eq!(first["purged"], 0, "{first}");
    assert_eq!(first["would_purge"], SUBJECTS, "{first}");
    assert!(first["purge_due_unix"].is_i64(), "the gate token is written: {first}");
    assert_eq!(tenp_count_tenants(&conn), SUBJECTS as i64 + 1, "nothing was deleted by the first sweep");

    step("report-only sweep done");
    // The scheduled sweep and admin.prune_now start together while calls keep flowing.
    let stop = Arc::new(AtomicBool::new(false));
    let busy_loop = tokio::spawn(loop_calls(server.base_url.clone(), live_key.clone(), tool.clone(), stop.clone()));
    let admin = McpClient::with_bearer(&server.base_url, "test-admin-key");
    let scheduled = {
        let db = server.state.db.clone();
        tokio::spawn(async move { db.prune_once().await })
    };
    let via_admin = admin.tools_call("admin.prune_now", json!({}));
    let (scheduled, via_admin) = tokio::join!(scheduled, via_admin);
    step("concurrent pair done");
    let scheduled = scheduled.unwrap().expect("scheduled sweep");
    let via_admin = extract_structured(&via_admin.expect("admin.prune_now"));
    let scheduled_busy = scheduled.busy;
    let admin_busy = via_admin["busy"] == true;
    assert!(scheduled_busy ^ admin_busy, "exactly one of the two reports busy: scheduled={scheduled_busy} admin={admin_busy}");
    let ran: Value = if scheduled_busy { via_admin[tenp::POLICY].clone() } else { scheduled.synthetic.clone().unwrap() };
    let busy: Value = if scheduled_busy { scheduled.synthetic.clone().unwrap() } else { via_admin[tenp::POLICY].clone() };
    assert_eq!(busy, json!({"busy": true}), "the loser changed nothing");
    let purged_first = ran["purged"].as_i64().unwrap();
    let remainder = SUBJECTS as i64 - purged_first;
    assert!(purged_first > 0 && remainder > 0, "the budget lets one sweep start but not finish: {ran}");
    assert_eq!(ran["held_over"], remainder, "held_over is the remainder: {ran}");
    assert_eq!(tenp_count_tenants(&conn), remainder + 1);

    // Later sweeps finish the rest: every subject goes exactly once, over two or more batches.
    let mut sweeps = 1;
    let mut batch_purged = vec![purged_first];
    let mut batch_walls: Vec<Value> = vec![ran["batch_wall_ms"].clone()];
    while tenp_count_tenants(&conn) > 1 {
        let report = server.state.db.prune_once().await.expect("later sweep");
        step("later sweep");
        let block = report.synthetic.expect("block");
        batch_purged.push(block["purged"].as_i64().unwrap());
        batch_walls.push(block["batch_wall_ms"].clone());
        sweeps += 1;
        assert!(sweeps < 60, "the purge makes progress every sweep: {batch_purged:?}");
    }
    stop.store(true, Ordering::Relaxed);
    let mut under_load = busy_loop.await.unwrap();
    let load_p95 = p95_ms(&mut under_load);

    assert!(batch_purged.len() >= 2, "deleted over two or more sweeps: {batch_purged:?}");
    assert_eq!(batch_purged.iter().sum::<i64>(), SUBJECTS as i64, "{batch_purged:?}");
    let audit: Vec<(String, i64)> = conn
        .prepare("SELECT tenant, COUNT(*) FROM admin_events WHERE action = 'tenant_purge_synthetic' GROUP BY tenant")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(audit.len(), SUBJECTS, "one journal row per subject");
    assert!(audit.iter().all(|(_, n)| *n == 1), "each subject was deleted exactly once");
    for (table, _) in &weights {
        assert_eq!(tenp_count_all(&conn, table), tenp_live_rows(&conn, table), "{table}: only the live tenant's rows remain");
    }
    for id in &ids {
        assert!(!tenp::exists(&conn, *id));
    }

    eprintln!("tenp_ac01 sweeps={sweeps} purged_per_sweep={batch_purged:?} batch_wall_ms={batch_walls:?}");
    eprintln!("tenp_ac01 loop p95: idle={baseline_p95} ms under purge={load_p95} ms ({} samples)", under_load.len());
    let relative_ceiling = (2 * baseline_p95).max(50);
    if perf_skipped() {
        eprintln!("perf skipped (load)");
    } else {
        assert!(load_p95 < 250, "loop p95 under 250 ms: {load_p95} ms (idle {baseline_p95} ms)");
        assert!(load_p95 < relative_ceiling, "loop p95 under max(2x idle, 50 ms) = {relative_ceiling} ms: {load_p95} ms");
    }
}

fn tenp_count_all(conn: &rusqlite::Connection, table: &str) -> i64 {
    conn.query_row(&format!("SELECT COUNT(*) FROM \"{table}\""), [], |r| r.get(0)).unwrap()
}

fn tenp_count_tenants(conn: &rusqlite::Connection) -> i64 {
    tenp_count_all(conn, "tenants")
}

/// Rows of `table` that belong to the live tenant (it publishes a tool and calls it).
fn tenp_live_rows(conn: &rusqlite::Connection, table: &str) -> i64 {
    conn.query_row(
        &format!("SELECT COUNT(*) FROM \"{table}\" WHERE tenant_id IN (SELECT id FROM tenants)"),
        [],
        |r| r.get(0),
    )
    .unwrap()
}
