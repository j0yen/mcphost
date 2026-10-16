//! mcphost-js-kind AC1 (P0) — Given the python bwrap profile and a 20-line js tool, When 50
//! cold calls run, Then the bench prints `cold_p95_ms=<n>`; `n > 300` fails
//! the test and the build stops with the measurement.
//!
//! The measurement is the PRD's falsifier: a QuickJS process started inside
//! the python kind's bwrap profile, loading a 20-line tool and answering one
//! bridge round trip (`mcphost.state.get`). Engine form measured: the
//! `mcphost-qjs` subprocess (kinds::js module doc).

use crate::common;
use mcphost::kinds::js::{self, CallParams, COLD_P95_BUDGET_MS};
use mcphost::sandbox::{self, NetworkMode, SidecarBridge};
use serde_json::{Value, json};
use std::sync::Arc;
use std::time::Instant;

const RUNS: usize = 50;

const TOOL: &str = r#"// 20-line tool: one bridge round trip, some arithmetic.
function total(items) {
  let sum = 0;
  for (const item of items) {
    sum += item.price * (item.qty || 1);
  }
  return sum;
}

function main(args) {
  const seen = mcphost.state.get("seen", 0);
  const items = args.items || [];
  return {
    total: total(items),
    count: items.length,
    seen: seen,
  };
}
"#;

/// Answers `mcphost.state.get` so the tool's bridge round trip completes.
struct StateBridge;

#[async_trait::async_trait]
impl SidecarBridge for StateBridge {
    async fn intercept(&self, line: &[u8]) -> Option<Vec<u8>> {
        let request: Value = serde_json::from_slice(line).ok()?;
        request.get("__mcphost_state__")?;
        Some(serde_json::to_vec(&json!({"ok": true, "result": {"found": true, "value": 7}})).unwrap())
    }
}

#[tokio::test]
async fn fifty_cold_calls_report_p95_under_budget() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let dir = common::TempDataDir::new();
    let params = CallParams {
        source: TOOL,
        timeout_s: 10,
        memory_mb: 256,
        network: NetworkMode::None,
        extra_env: Vec::new(),
        isolation: sandbox::detect_mechanism(),
    };
    let payload = json!({"args": {"items": [{"price": 2, "qty": 3}, {"price": 5}]}});
    let cancel = Arc::new(mcphost::runs::CancelSlot::default());

    let mut ms = Vec::with_capacity(RUNS);
    for _ in 0..RUNS {
        let started = Instant::now();
        let outcome = js::cold_call(&dir.0, &params, &payload, &StateBridge, &cancel)
            .await
            .expect("cold call");
        ms.push(started.elapsed().as_millis() as u64);
        let sandbox::SandboxOutcome::Exited { stdout, .. } = outcome else {
            panic!("cold call did not exit cleanly: {outcome:?}");
        };
        let envelope: Value = serde_json::from_slice(&stdout).expect("envelope json");
        assert_eq!(
            envelope["result"],
            json!({"total": 11, "count": 2, "seen": 7}),
            "the 20-line tool must compute its result through one bridge round trip: {envelope}"
        );
    }
    ms.sort_unstable();
    let p95 = ms[(RUNS * 95).div_ceil(100) - 1];
    println!(
        "cold_p95_ms={p95} engine=subprocess(mcphost-qjs in the python bwrap profile) min_ms={} max_ms={}",
        ms[0],
        ms[RUNS - 1]
    );
    assert!(
        p95 <= COLD_P95_BUDGET_MS,
        "cold_p95_ms={p95} exceeds the {COLD_P95_BUDGET_MS} ms budget: js has no latency edge as a subprocess; build stops with this measurement (fallback: in-process rquickjs, PRD open questions)"
    );
}
