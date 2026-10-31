//! PRD-mcphost-synthetic-tenant-purge
//! AC1 (P0) — Given the § Fixture (6,000 due subjects, 20 heavy) and a
//! host.tool_call loop on a live tenant, When the scheduled sweep and
//! admin.prune_now start together with a budget one sweep cannot finish, Then
//! one reports busy, every subject is deleted exactly once over two or more
//! batches, held_over equals the remainder, loop p95 stays under 250 ms and
//! under max(2x the idle baseline, 50 ms), and per-batch wall time in the PR
//! body (printed here, one line per batch).

use crate::common::tenp::{self, DAY};
use crate::common::{ADMIN_KEY, EnvGuard, McpClient, TestServer, extract_structured, publish, signup};
use serde_json::{Value, json};
use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const NORMAL: i64 = 5_980;
const HEAVY: i64 = 20;
const SUBJECTS: i64 = NORMAL + HEAVY;
const HEAVY_KV_ROWS: i64 = 50_000;
/// Idle-baseline sampling window before any sweep starts.
const BASELINE_MS: u64 = 10_000;

fn p95(mut samples: Vec<u64>) -> u64 {
    assert!(!samples.is_empty(), "no latency samples");
    samples.sort_unstable();
    samples[(samples.len() * 95 / 100).min(samples.len() - 1)]
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn tenp_ac01_concurrent_sweeps_purge_every_subject_exactly_once() {
    // A budget of zero: every sweep runs one batch and stops, so no sweep can finish the 6,000.
    let _budget = EnvGuard::set(tenp::BUDGET_ENV, "0").await;
    let server = TestServer::start().await;
    server.state.db.seed_retention_policy_from_env().await.expect("seed");
    let (live_ns, key) = signup(&server.base_url, "Tenp Live").await;
    let live = McpClient::with_bearer(&server.base_url, &key);
    publish(&live, "ping", "echo", json!({"schema": {"type": "object"}})).await;

    let mut conn = rusqlite::Connection::open(server.data_dir.0.join("mcphost.db")).expect("raw db");
    conn.busy_timeout(Duration::from_secs(120)).expect("busy timeout");
    // The fixture plants child rows without their parents.
    conn.pragma_update(None, "foreign_keys", "OFF").expect("foreign_keys off");
    // The loop outruns the free plan's 500 calls a day.
    conn.execute("UPDATE tenants SET plan = 'pro' WHERE namespace = ?1", [&live_ns]).expect("pro plan");
    let now = mcphost::state::now_unix();
    let tx = conn.transaction().unwrap();
    tenp::set_floor(&tx, now - 30 * DAY);
    tenp::insert_subjects(&tx, "tenp-s", NORMAL, now - 60 * DAY, now - 20 * DAY, "self_offboard");
    tenp::insert_subjects(&tx, "tenp-h", HEAVY, now - 60 * DAY, now - 20 * DAY, "idle_expired");
    let empty = tenp::fill_children(&tx, "tenp-%", tenp::HEAVY_TABLE_ROWS);
    tenp::fill_table(&tx, "tenant_state_kv", HEAVY_KV_ROWS, "tenp-h%");
    tenp::seed_pre_prd_prune_log(&tx, now - 5 * DAY);
    tx.commit().unwrap();
    assert!(empty.is_empty(), "tables that took no fixture row: {empty:?}");
    let (min_id, max_id): (i64, i64) = conn
        .query_row("SELECT MIN(id), MAX(id) FROM tenants WHERE namespace LIKE 'tenp-%'", [], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap();
    assert_eq!(max_id - min_id + 1, SUBJECTS);
    let expected_rows = NORMAL * 375 + HEAVY * HEAVY_KV_ROWS;
    assert!(tenp::heavy_row_count(&conn, "tenp-%") >= expected_rows * 9 / 10, "the fixture is prod-sized");

    // Sweep 0: the first one that carries the block only reports.
    let first = server.state.db.prune_once().await.expect("first sweep");
    assert_eq!(first.synthetic["purged"], 0, "{}", first.synthetic);
    assert_eq!(first.synthetic["would_purge"], SUBJECTS, "{}", first.synthetic);
    assert!(first.synthetic["purge_due_unix"].is_i64(), "{}", first.synthetic);
    assert_eq!(tenp::count(&conn, "SELECT COUNT(*) FROM tenants WHERE namespace LIKE 'tenp-%'"), SUBJECTS);

    // The host.tool_call loop on the live tenant, sampled before and during the sweeps.
    let stop = Arc::new(AtomicBool::new(false));
    let samples: Arc<Mutex<Vec<(Instant, u64)>>> = Arc::default();
    let looper = tokio::spawn({
        let (stop, samples) = (stop.clone(), samples.clone());
        async move {
            while !stop.load(Ordering::SeqCst) {
                let started = Instant::now();
                live.tools_call("host.tool_call", json!({"name": "ping", "args": {}})).await.expect("loop call");
                let ms = started.elapsed().as_millis() as u64;
                samples.lock().unwrap().push((started, ms));
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        }
    });
    tokio::time::sleep(Duration::from_millis(BASELINE_MS)).await;
    let sweeps_start = Instant::now();

    // The scheduled sweep and admin.prune_now, together.
    let admin = McpClient::with_bearer(&server.base_url, ADMIN_KEY);
    let (scheduled, now_call) = tokio::join!(server.state.db.prune_once(), admin.tools_call("admin.prune_now", json!({})));
    let scheduled = scheduled.expect("scheduled sweep").synthetic;
    let now_call = extract_structured(&now_call.expect("admin.prune_now"))[tenp::POLICY].clone();
    let blocks: Vec<&Value> = vec![&scheduled, &now_call];
    let busy: Vec<bool> = blocks.iter().map(|b| b["busy"] == json!(true)).collect();
    assert_eq!(busy.iter().filter(|b| **b).count(), 1, "exactly one reports busy: {scheduled} / {now_call}");
    let winner = blocks[busy.iter().position(|b| !*b).unwrap()];
    let first_batch = winner["purged"].as_i64().unwrap();
    assert!(first_batch > 0 && first_batch < SUBJECTS, "one sweep cannot finish: {winner}");
    assert_eq!(winner["held_over"], SUBJECTS - first_batch, "held_over is the remainder: {winner}");

    // Drain: sweeps until nothing is due.
    let mut batches: Vec<(i64, i64)> = Vec::new();
    let collect = |block: &Value, batches: &mut Vec<(i64, i64)>| {
        for b in block["batches"].as_array().into_iter().flatten() {
            eprintln!("tenp_ac01 batch detail {b}");
            batches.push((b["purged"].as_i64().unwrap(), b["wall_ms"].as_i64().unwrap()));
        }
    };
    collect(winner, &mut batches);
    let mut total = first_batch;
    for _ in 0..40 {
        if total == SUBJECTS {
            break;
        }
        eprintln!("tenp_ac01 drain sweep, {total} purged so far, {} ms in", sweeps_start.elapsed().as_millis());
        let block = server.state.db.prune_once().await.expect("drain sweep").synthetic;
        let purged = block["purged"].as_i64().unwrap();
        assert!(purged > 0, "a sweep with a due subject purges: {block}");
        assert_eq!(block["held_over"], SUBJECTS - total - purged, "{block}");
        collect(&block, &mut batches);
        total += purged;
    }
    let sweeps_ms = sweeps_start.elapsed().as_millis() as u64;
    stop.store(true, Ordering::SeqCst);
    looper.await.expect("loop task");

    assert_eq!(total, SUBJECTS, "every subject purged");
    assert!(batches.len() >= 2, "over two or more batches: {batches:?}");
    for (i, (purged, wall_ms)) in batches.iter().enumerate() {
        eprintln!("tenp_ac01 batch {}: purged {purged} in {wall_ms} ms", i + 1);
    }
    eprintln!("tenp_ac01 sweeps wall {sweeps_ms} ms");

    // Exactly once: one admin_events row per subject, no namespace twice, no row left behind.
    let audited: Vec<String> = conn
        .prepare("SELECT tenant FROM admin_events WHERE action = 'tenant_purge_synthetic'")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(audited.len() as i64, SUBJECTS);
    assert_eq!(audited.iter().collect::<HashSet<_>>().len() as i64, SUBJECTS, "no subject audited twice");
    assert_eq!(tenp::count(&conn, "SELECT COUNT(*) FROM tenants WHERE namespace LIKE 'tenp-%'"), 0);
    assert_eq!(tenp::heavy_row_count(&conn, "tenp-%"), 0);
    for table in tenp::cascaded_tables(&conn) {
        let cols: Vec<String> = conn
            .prepare("SELECT \"from\" FROM pragma_foreign_key_list(?1) WHERE \"table\" = 'tenants' AND on_delete = 'CASCADE'")
            .unwrap()
            .query_map([&table], |r| r.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        for col in cols {
            let left = tenp::count(
                &conn,
                &format!("SELECT COUNT(*) FROM \"{table}\" WHERE \"{col}\" BETWEEN {min_id} AND {max_id}"),
            );
            assert_eq!(left, 0, "{table}.{col} still holds rows of a purged subject");
        }
    }

    // Latency: the loop on the live tenant never stalls behind a subject's transaction.
    let all = samples.lock().unwrap().clone();
    let baseline: Vec<u64> = all.iter().filter(|(at, _)| *at < sweeps_start).map(|(_, ms)| *ms).collect();
    let during: Vec<u64> = all.iter().filter(|(at, _)| *at >= sweeps_start).map(|(_, ms)| *ms).collect();
    let (base_p95, during_p95) = (p95(baseline), p95(during));
    eprintln!("tenp_ac01 loop p95: baseline {base_p95} ms, during sweeps {during_p95} ms");
    assert!(during_p95 < 250, "p95 {during_p95} ms >= 250 ms");
    let relative_bound = (2 * base_p95).max(50);
    assert!(during_p95 < relative_bound, "p95 {during_p95} ms >= max(2x baseline {base_p95}, 50) = {relative_bound}");
}
