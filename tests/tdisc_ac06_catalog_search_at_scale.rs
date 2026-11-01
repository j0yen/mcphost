//! PRD-mcphost-trusted-discovery-catalog-receipts AC6 (P0) -- with 1,000 public
//! tools over 300 plain owners and 100,000 `calls` rows, 8 concurrent callers
//! searching (no `q`, and a `q` that matches nothing, both with limit 50) get
//! 50 entries with `view.owners` counting the owners, and `tools: []` with the
//! `view` still present; p95 over 20 runs per caller stays under 250 ms.
//!
//! The p95 is skipped under `MCPHOST_PERF_SKIP=1` like every timing assertion
//! (the gate sets it on a loaded host and re-runs on a quiet one).

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup_plain};
use rusqlite::Connection;
use serde_json::{Value, json};
use std::time::Instant;

const OWNERS: usize = 300;
const TOOLS: usize = 1_000;
const CALLS: usize = 100_000;
const CALLERS: usize = 8;
const RUNS: usize = 20;
const P95_BUDGET_MS: f64 = 250.0;

/// Deterministic pseudo-random stream (no new dependency for a seed).
fn next(state: &mut u64) -> u64 {
    *state = state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
    *state >> 33
}

fn p95(samples: &mut [f64]) -> f64 {
    samples.sort_by(|a, b| a.partial_cmp(b).expect("finite"));
    samples[(samples.len() * 95).div_ceil(100) - 1]
}

async fn seed_catalog(server: &TestServer) -> Vec<String> {
    let mut clients = Vec::new();
    for i in 0..OWNERS {
        let (ns, key) = signup_plain(&server.base_url, &format!("Tdisc AC6 Owner {i}")).await;
        clients.push((ns, McpClient::with_bearer(&server.base_url, &key)));
    }
    for t in 0..TOOLS {
        let (ns, client) = &clients[t % OWNERS];
        let name = format!("scale_{t}");
        client
            .tools_call("host.tool_publish", json!({"name": name, "kind": "echo", "spec": {"schema": {"type": "object"}}}))
            .await
            .unwrap_or_else(|e| panic!("publish {ns}.{name}: {} {}", e.code, e.message));
        client
            .tools_call("host.tool_share", json!({"name": name, "visibility": "public", "description": format!("tdisc ac6 tool {t}")}))
            .await
            .unwrap_or_else(|e| panic!("share {ns}.{name}: {} {}", e.code, e.message));
    }
    clients.into_iter().map(|(ns, _)| ns).collect()
}

/// 100,000 `calls` rows, every one a call by a tenant other than the owner (the
/// worst case for the receipts aggregate), a few failures and a tenth older than
/// the 30-day window.
fn seed_calls(server: &TestServer) {
    let mut db = Connection::open(server.data_dir.0.join("mcphost.db")).unwrap();
    let tools: Vec<(i64, String)> = db
        .prepare("SELECT tenant_id, name FROM tools WHERE visibility = 'public'")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(tools.len(), TOOLS);
    let tenants: Vec<i64> = db
        .prepare("SELECT id FROM tenants")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    let now = mcphost::state::now_unix();
    let mut rng = 42u64;
    let tx = db.transaction().unwrap();
    {
        let mut insert = tx
            .prepare(
                "INSERT INTO calls (tenant_id, tool_name, started_at, started_unix, duration_ms, ok, caller_tenant_id) \
                 VALUES (?1, ?2, 'x', ?3, 1, ?4, ?5)",
            )
            .unwrap();
        for _ in 0..CALLS {
            let (owner, name) = &tools[next(&mut rng) as usize % tools.len()];
            let mut caller = tenants[next(&mut rng) as usize % tenants.len()];
            if caller == *owner {
                caller = tenants[(next(&mut rng) as usize + 1) % tenants.len()];
            }
            let age = if next(&mut rng) % 10 == 0 { 31 * 86_400 + 60 } else { 60 + (next(&mut rng) % (29 * 86_400)) as i64 };
            let ok = (next(&mut rng) % 20 != 0) as i64;
            insert.execute(rusqlite::params![owner, name, now - age, ok, caller]).unwrap();
        }
    }
    tx.commit().unwrap();
}

#[tokio::test]
async fn tdisc_ac06() {
    // 1,000 shares over 300 owners is 3-4 per owner: past the free plan's 3.
    let server = TestServer::start_with_plans(|catalog| {
        for plan in &mut catalog.plans {
            plan.shared_tools_max = plan.shared_tools_max.max(10);
        }
    })
    .await;
    seed_catalog(&server).await;
    seed_calls(&server);

    let mut callers = Vec::new();
    for i in 0..CALLERS {
        let (_ns, key) = signup_plain(&server.base_url, &format!("Tdisc AC6 Caller {i}")).await;
        callers.push(key);
    }

    // correctness, once
    let probe = McpClient::with_bearer(&server.base_url, &callers[0]);
    let all = extract_structured(&probe.tools_call("host.catalog.search", json!({"limit": 50})).await.unwrap());
    assert_eq!(all["tools"].as_array().map(Vec::len), Some(50), "no q: {all}");
    assert_eq!(all["view"], json!({"include_synthetic": false, "owners": OWNERS}), "{all}");
    let none = extract_structured(
        &probe.tools_call("host.catalog.search", json!({"q": "tdisc-ac6-matches-nothing", "limit": 50})).await.unwrap(),
    );
    assert_eq!(none["tools"], json!([]), "{none}");
    assert_eq!(none["view"], json!({"include_synthetic": false, "owners": 0}), "{none}");

    // a new cross-tenant call, written by any statement, shows in the next read
    let top = &all["tools"][0];
    let before = top["calls_30d"]["n"].as_i64().expect("n");
    let (owner_ns, tool) = top["name"].as_str().and_then(|n| n.split_once('.')).expect("ns.tool");
    let db = Connection::open(server.data_dir.0.join("mcphost.db")).unwrap();
    db.execute(
        "INSERT INTO calls (tenant_id, tool_name, started_at, started_unix, duration_ms, ok, caller_tenant_id) \
         SELECT o.id, ?2, 'x', ?3, 1, 1, c.id FROM tenants o, tenants c WHERE o.namespace = ?1 AND c.id != o.id AND c.synthetic IS NULL AND c.origin != 'synthetic' LIMIT 1",
        rusqlite::params![owner_ns, tool, mcphost::state::now_unix()],
    )
    .unwrap();
    let again = extract_structured(&probe.tools_call("host.catalog.search", json!({"q": tool, "limit": 50})).await.unwrap());
    let entry = again["tools"].as_array().unwrap().iter().find(|t| t["name"] == top["name"]).expect("same tool");
    assert_eq!(entry["calls_30d"]["n"].as_i64(), Some(before + 1), "{entry}");

    // p95 over RUNS runs per caller, the callers concurrent
    for args in [json!({"limit": 50}), json!({"q": "tdisc-ac6-matches-nothing", "limit": 50})] {
        let mut tasks = Vec::new();
        for key in &callers {
            let (base, key, args) = (server.base_url.clone(), key.clone(), args.clone());
            tasks.push(tokio::spawn(async move {
                let client = McpClient::with_bearer(&base, &key);
                let mut ms = Vec::with_capacity(RUNS);
                for _ in 0..RUNS {
                    let started = Instant::now();
                    let result: Value = extract_structured(&client.tools_call("host.catalog.search", args.clone()).await.unwrap());
                    ms.push(started.elapsed().as_secs_f64() * 1000.0);
                    assert!(result["view"].is_object(), "{result}");
                }
                ms
            }));
        }
        let mut samples = Vec::new();
        for task in tasks {
            samples.extend(task.await.expect("caller task"));
        }
        let measured = p95(&mut samples);
        eprintln!("tdisc_ac06 {args}: p95 = {measured:.1} ms over {} samples (budget {P95_BUDGET_MS} ms)", samples.len());
        if common::perf_skipped() {
            eprintln!("perf skipped (load)");
            continue;
        }
        assert!(measured < P95_BUDGET_MS, "catalog.search {args} p95 {measured:.1} ms is over the {P95_BUDGET_MS} ms budget");
    }
}
