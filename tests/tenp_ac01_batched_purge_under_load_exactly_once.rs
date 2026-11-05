//! PRD-mcphost-synthetic-tenant-purge
//! AC1 (P0) — Given the § Fixture (6,000 due subjects, 20 heavy) and a
//! `host.tool_call` loop on a live tenant, When the scheduled sweep and
//! `admin.prune_now` start together with a budget one sweep cannot finish,
//! Then one reports busy, every subject is deleted exactly once over two or
//! more batches, `held_over` equals the remainder, loop p95 stays under 250 ms
//! and under max(2x the idle baseline, 50 ms), and per-batch wall time is
//! printed for the PR body.

use crate::common::tenp::{self, DAY};
use crate::common::tenx;
use crate::common::{ADMIN_KEY, EnvGuard, McpClient, TestServer, extract_structured, publish, signup_and_make_pro};
use async_trait::async_trait;
use mcphost::kinds::{Kind, KindError, KindRegistry, ToolDescriptor};
use serde_json::{Value, json};
use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const SUBJECTS: usize = 6_000;
const HEAVY: usize = 20;
const HEAVY_KV: i64 = 50_000;
const PURGE_BATCH: usize = 500;

/// Records each tenant the purge stage cleans up after.
struct HookKind {
    removed: Arc<Mutex<Vec<i64>>>,
}

#[async_trait]
impl Kind for HookKind {
    fn contract(&self) -> &'static mcphost::kinds::contract::KindContract {
        &mcphost::kinds::contract::EMPTY_CONTRACT
    }
    fn name(&self) -> &'static str {
        "tenphook"
    }
    fn validate(&self, _spec: &Value) -> Result<(), KindError> {
        Ok(())
    }
    fn describe(&self, _spec: &Value) -> ToolDescriptor {
        ToolDescriptor {
            name: "tenphook".to_string(),
            description: "records tenant removal".to_string(),
            input_schema: json!({"type": "object"}),
        }
    }
    async fn call(&self, _spec: &Value, _args: Value, _ctx: &mcphost::kinds::CallCtx) -> Result<Value, KindError> {
        Ok(json!({}))
    }
    async fn on_tenant_removed(&self, tenant_id: i64) {
        self.removed.lock().unwrap().push(tenant_id);
    }
}

/// A `host.tool_call` loop on a live tenant; its per-call latencies in microseconds.
struct Loop {
    stop: Arc<AtomicBool>,
    samples: Arc<Mutex<Vec<u64>>>,
    handle: tokio::task::JoinHandle<()>,
}

impl Loop {
    fn start(base_url: String, key: String) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let samples = Arc::new(Mutex::new(Vec::new()));
        let handle = tokio::spawn({
            let (stop, samples) = (stop.clone(), samples.clone());
            async move {
                let client = McpClient::with_bearer(&base_url, &key);
                while !stop.load(Ordering::SeqCst) {
                    let started = Instant::now();
                    client
                        .tools_call("host.tool_call", json!({"name": "echoer", "args": {}}))
                        .await
                        .expect("loop call");
                    samples.lock().unwrap().push(started.elapsed().as_micros() as u64);
                    tokio::time::sleep(Duration::from_millis(2)).await;
                }
            }
        });
        Self { stop, samples, handle }
    }

    /// Stops the loop and returns its samples, sorted.
    async fn finish(self) -> Vec<u64> {
        self.stop.store(true, Ordering::SeqCst);
        self.handle.await.expect("loop task");
        let mut samples = self.samples.lock().unwrap().clone();
        samples.sort_unstable();
        samples
    }
}

fn p95_ms(sorted_us: &[u64]) -> f64 {
    let idx = ((sorted_us.len() as f64) * 0.95).ceil() as usize;
    sorted_us[idx.saturating_sub(1).min(sorted_us.len() - 1)] as f64 / 1000.0
}

/// The § Fixture: `SUBJECTS` due subjects with prod's per-subject row ratio,
/// `HEAVY` of them with `HEAVY_KV` `tenant_state_kv` rows, and one row in every
/// other table that names a tenant.
fn build_fixture(conn: &rusqlite::Connection, now: i64) -> Vec<i64> {
    let last_seen = now - 100 * DAY;
    tenx::set_floor(conn, last_seen);
    conn.execute_batch("BEGIN").unwrap();
    let ids: Vec<i64> = (0..SUBJECTS).map(|i| tenp::insert(conn, &tenp::Row::subject(&format!("tenp-s{i}"), last_seen))).collect();
    let pairs: Vec<(i64, i64)> = ids.iter().map(|id| (*id, *id)).collect();
    tenp::set_ids(conn, &pairs);
    let counted = |conn: &rusqlite::Connection, sql: &str, n: i64| {
        conn.execute(&format!("WITH RECURSIVE nums(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM nums WHERE i < {n}) {sql}"), [])
            .unwrap_or_else(|e| panic!("{sql}: {e}"));
    };
    counted(conn, "INSERT INTO runs (id, tenant_id, tool_name, trigger, status) \
        SELECT 'r' || t.id || '_' || nums.i, t.id, 'echoer', 'manual', 'ok' FROM tenp_ids t, nums", 120);
    counted(conn, "INSERT INTO logs (tenant_id, tool_name, ts, line) \
        SELECT t.id, 'echoer', strftime('%Y-%m-%dT%H:%M:%SZ','now'), 'line ' || nums.i FROM tenp_ids t, nums", 110);
    counted(conn, "INSERT INTO tenant_state_kv (tenant_id, key, value_json, updated_unix) \
        SELECT t.id, 'k' || nums.i, '{}', CAST(strftime('%s','now') AS INTEGER) FROM tenp_ids t, nums", 100);
    counted(conn, "INSERT INTO trigger_receipts (tenant_id, trigger_id, tool_name, run_id, source, started_unix, duration_ms, status) \
        SELECT t.id, 'tr', 'echoer', 'run' || nums.i, 'cron', CAST(strftime('%s','now') AS INTEGER), 1, 'ok' FROM tenp_ids t, nums", 37);
    counted(conn, "INSERT INTO host_tool_usage (tenant_id, tool_name, first_used_unix) \
        SELECT t.id, 'tool' || nums.i, CAST(strftime('%s','now') AS INTEGER) FROM tenp_ids t, nums", 8);
    let spread = SUBJECTS / HEAVY;
    let heavy: Vec<(i64, i64)> = (0..HEAVY).map(|i| (ids[i * spread + 1], ids[i * spread + 1])).collect();
    tenp::set_ids(conn, &heavy);
    counted(conn, "INSERT INTO tenant_state_kv (tenant_id, key, value_json, updated_unix) \
        SELECT t.id, 'heavy' || nums.i, '{}', CAST(strftime('%s','now') AS INTEGER) FROM tenp_ids t, nums", HEAVY_KV - 100);
    tenp::set_ids(conn, &pairs);
    // A lifecycle event dates the due stamp; a subject disabled before the PRD has none.
    // `doc_chunks_fts` is a standalone FTS5 table the delete scans (392 rows on prod): it
    // gets rows for the heavy subjects only, not one per subject.
    let skip = [
        "tenants", "runs", "logs", "tenant_state_kv", "trigger_receipts", "host_tool_usage", "tenant_lifecycle_events",
        "doc_chunks_fts",
    ];
    for table in tenp::tenant_tables(conn).keys() {
        if !skip.contains(&table.as_str()) {
            tenp::plant(conn, table, &[]);
        }
    }
    tenp::set_ids(conn, &heavy);
    tenp::plant(conn, "doc_chunks_fts", &[]);
    // Keep the server's own workers off the fixture: the docs indexer ticks every
    // tenant with a pending document or an unpaused index source.
    conn.execute_batch("UPDATE documents SET seq = 0; UPDATE index_sources SET paused = 1; COMMIT").unwrap();
    ids
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn tenp_ac01_batched_purge_under_load_exactly_once() {
    // A budget one sweep cannot finish: each sweep stops after its first batch.
    let _env = EnvGuard::set(tenp::BUDGET_ENV, "0").await;
    let removed = Arc::new(Mutex::new(Vec::new()));
    let mut kinds = KindRegistry::with_builtin();
    kinds.register(Arc::new(HookKind { removed: removed.clone() }));
    let server = TestServer::start_with_kinds(kinds).await;
    server.state.db.seed_retention_policy_from_env().await.expect("seed");
    let conn = tenp::open_raw(&server.data_dir.0);
    let now = mcphost::state::now_unix();
    let ids = build_fixture(&conn, now);
    tenp::seed_pre_prd_row(&conn, now);
    assert_eq!(tenp::count(&conn, "SELECT COUNT(*) FROM tenant_state_kv"), 6_000 * 100 + HEAVY as i64 * (HEAVY_KV - 100));

    // A live tenant with a published tool, called in a loop.
    let (_ns, key, _id) = signup_and_make_pro(&server, "Tenp Loop", "cus_tenp_loop").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    publish(&client, "echoer", "echo", json!({"schema": {"type": "object"}})).await;

    let baseline_loop = Loop::start(server.base_url.clone(), key.clone());
    tokio::time::sleep(Duration::from_secs(10)).await;
    let baseline_p95 = p95_ms(&baseline_loop.finish().await);

    let load = Loop::start(server.base_url.clone(), key.clone());
    let admin = McpClient::with_bearer(&server.base_url, ADMIN_KEY);
    let mut pairs_run = 0usize;
    let mut busy_pairs = 0usize;
    let mut purged_total = 0i64;
    let mut batch_ms: Vec<u64> = Vec::new();
    while tenp::count(&conn, "SELECT COUNT(*) FROM tenants WHERE namespace LIKE 'tenp-s%'") > 0 {
        assert!(pairs_run < 40, "the purge did not finish");
        let (direct, via_admin) = tokio::join!(
            server.state.db.prune_once(),
            admin.tools_call("admin.prune_now", json!({}))
        );
        let sweep_error = || tenp::latest_error(&conn);
        let direct = direct.unwrap_or_else(|e| panic!("scheduled sweep: {e}: {}", sweep_error())).synthetic;
        let via_admin = extract_structured(&via_admin.unwrap_or_else(|e| panic!("admin.prune_now: {e:?}: {}", sweep_error())))
            ["synthetic_implicit_tenants"]
            .clone();
        let blocks = [direct, via_admin];
        let busy = blocks.iter().filter(|b| b["busy"] == true).count();
        let ran: Vec<&Value> = blocks.iter().filter(|b| b["busy"] != true).collect();
        assert_eq!(busy + ran.len(), 2);
        assert!(!ran.is_empty(), "one sweep always runs: {blocks:?}");
        let ran = ran[0];
        let purged = ran["purged"].as_i64().expect("purged");
        if pairs_run == 0 {
            // The R1 gate: the first sweep that carries the purge block deletes nothing.
            assert_eq!(purged, 0, "first armed sweep reports only: {ran}");
            assert_eq!(ran["would_purge"], SUBJECTS, "{ran}");
        } else {
            assert_eq!(busy, 1, "the second starter reports busy while the first purges: {blocks:?}");
            busy_pairs += 1;
            assert_eq!(purged, PURGE_BATCH as i64, "budget 0: one batch per sweep: {ran}");
            purged_total += purged;
            let remainder = SUBJECTS as i64 - purged_total;
            assert_eq!(ran["held_over"], remainder, "held_over is the remainder: {ran}");
            batch_ms.extend(ran["batch_wall_ms"].as_array().expect("batch_wall_ms").iter().map(|v| v.as_u64().unwrap()));
            let rows = tenp::count(&conn, "SELECT COUNT(*) FROM prune_log WHERE json_extract(deleted_json, '$.synthetic_implicit_tenants.busy') = 1");
            assert!(rows >= busy_pairs as i64, "each busy sweep wrote its own prune_log row");
        }
        pairs_run += 1;
    }
    let loaded = load.finish().await;
    let loaded_p95 = p95_ms(&loaded);

    // Exactly once, over two or more batches.
    assert_eq!(purged_total, SUBJECTS as i64);
    assert!(busy_pairs >= 2, "two or more batches: {busy_pairs}");
    let audit = tenp::count(&conn, "SELECT COUNT(*) FROM admin_events WHERE action = 'tenant_purge_synthetic'");
    assert_eq!(audit, SUBJECTS as i64, "one audit row per subject");
    assert_eq!(
        tenp::count(&conn, "SELECT COUNT(DISTINCT tenant) FROM admin_events WHERE action = 'tenant_purge_synthetic'"),
        SUBJECTS as i64
    );
    let cleaned = removed.lock().unwrap().clone();
    let unique: HashSet<i64> = cleaned.iter().copied().collect();
    assert_eq!(cleaned.len(), unique.len(), "no cleanup ran twice");
    assert_eq!(unique, ids.iter().copied().collect::<HashSet<_>>(), "every subject was cleaned up");
    for table in ["runs", "logs", "tenant_state_kv", "trigger_receipts", "host_tool_usage"] {
        let left = tenp::count(&conn, &format!("SELECT COUNT(*) FROM {table} WHERE tenant_id IN (SELECT id FROM tenants WHERE namespace LIKE 'tenp-s%')"));
        assert_eq!(left, 0, "{table}");
    }
    for (table, cols) in tenp::tenant_tables(&conn) {
        if table == "tenants" {
            continue;
        }
        for c in cols.iter().filter(|c| c.fk.as_deref() == Some("CASCADE")) {
            let live: i64 = tenp::count(&conn, &format!("SELECT COUNT(*) FROM {table} WHERE {} IN ({})", c.column, ids.iter().take(50).map(|i| i.to_string()).collect::<Vec<_>>().join(",")));
            assert_eq!(live, 0, "{table}.{}", c.column);
        }
    }

    eprintln!("tenp_ac01: per-batch wall ms {batch_ms:?}");
    eprintln!("tenp_ac01: baseline p95 {baseline_p95:.1} ms, loaded p95 {loaded_p95:.1} ms over {} calls", loaded.len());
    let bound_ms = (2.0 * baseline_p95).max(50.0);
    // The gate sets MCPHOST_PERF_SKIP=1 on a loaded host and re-runs this on a quiet one: a
    // latency percentile measured next to other test binaries measures the builder.
    if crate::common::perf_skipped() {
        eprintln!("perf skipped (load)");
        return;
    }
    assert!(loaded_p95 < 250.0, "loop p95 {loaded_p95} ms must stay under 250 ms");
    assert!(loaded_p95 <= bound_ms, "loop p95 {loaded_p95} ms must stay under max(2x idle {baseline_p95}, 50) = {bound_ms}");
}
