//! PRD-mcphost-session-journal-v2
//! AC5 (P0, the falsifier) — Given 1000 warm echo calls with and without the
//! two values written, When `record_call_traced`'s p95 is compared on the
//! gate box, Then the delta is at most 1 ms and both numbers are printed.
//!
//! The "with" arm pays the whole new cost of a published-tool call: the
//! masked-arguments fingerprint (`note_call_args`: one `Redactor` walk and one
//! sha256 over a 12-key argument object, one tenant secret in scope) and the
//! two extra bound parameters of the insert. The "without" arm writes the same
//! row with neither. The arms alternate call by call so drift on a shared host
//! lands on both; up to three rounds are measured and the best judged, as
//! `ctrace_ac01` does. Skipped under `MCPHOST_PERF_SKIP=1` like every
//! `perf_budget!` test.

use crate::common::{self, TestServer, signup};
use mcphost::calllog;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::time::Instant;

const CALLS_PER_ARM: usize = 1000;
const BUDGET_MS: f64 = 1.0;
const ROUNDS: usize = 3;

fn p95_ms(samples: &mut [f64]) -> f64 {
    samples.sort_by(|a, b| a.partial_cmp(b).expect("finite"));
    let rank = ((samples.len() as f64) * 0.95).ceil() as usize;
    samples[rank.saturating_sub(1).min(samples.len() - 1)]
}

async fn write_row(db: &mcphost::db::Db, tenant_id: i64) {
    db.record_call_traced(
        tenant_id,
        "echo".to_string(),
        1,
        true,
        None,
        None,
        None,
        "ok",
        "external".to_string(),
        None,
        None,
        None,
        None,
        None,
        None,
        "key".to_string(),
        None,
        None,
        None,
        None,
    )
    .await
    .expect("record call");
}

#[tokio::test]
async fn jrnl_ac05_two_values_cost_at_most_one_ms_p95() {
    let server = TestServer::start().await;
    let (ns, _key) = signup(&server.base_url, "Journal Cost Tenant").await;
    let tenant = server.state.db.find_tenant_by_namespace(ns).await.expect("find").expect("tenant");
    let db = &server.state.db;
    let secrets: HashMap<String, String> = HashMap::from([("VAULT".to_string(), "s3cr3t-value-8".to_string())]);
    let args: Value = (0..12).fold(json!({}), |mut acc, i| {
        acc[format!("field_{i}")] = json!({"n": i, "label": format!("value {i}"), "tags": ["a", "b", "c"]});
        acc
    });
    // Warm the connection and the statement cache.
    for _ in 0..50 {
        write_row(db, tenant.id).await;
    }

    let mut best = f64::MAX;
    for round in 1..=ROUNDS {
        let mut with_values = Vec::with_capacity(CALLS_PER_ARM);
        let mut without_values = Vec::with_capacity(CALLS_PER_ARM);
        for i in 0..CALLS_PER_ARM * 2 {
            let with = i % 2 == 0;
            let started = Instant::now();
            if with {
                calllog::with_session(
                    Some("0123456789abcdef0123456789abcdef.0123456789abcdef0123456789abcdef".to_string()),
                    calllog::with_args_cell(async {
                        calllog::note_call_args(&args, &secrets);
                        write_row(db, tenant.id).await;
                    }),
                )
                .await;
            } else {
                write_row(db, tenant.id).await;
            }
            let ms = started.elapsed().as_secs_f64() * 1000.0;
            if with { &mut with_values } else { &mut without_values }.push(ms);
        }
        let p95_with = p95_ms(&mut with_values);
        let p95_without = p95_ms(&mut without_values);
        let delta = p95_with - p95_without;
        println!(
            "jrnl falsifier round {round}: record_call_traced p95 with args_sha256+session_id = {p95_with:.3} ms, without = {p95_without:.3} ms, delta = {delta:.3} ms (budget {BUDGET_MS} ms)"
        );
        best = best.min(delta);
        if best <= BUDGET_MS || common::perf_skipped() {
            break;
        }
    }

    if common::perf_skipped() {
        eprintln!("perf skipped (load)");
        return;
    }
    assert!(
        best <= BUDGET_MS,
        "writing args_sha256 and session_id added {best:.3} ms at p95 in its best of {ROUNDS} rounds, over the {BUDGET_MS} ms budget"
    );
}
