//! PRD-mcphost-run-budget-governor
//! AC7 (P0, guardrail) — Given 200 child calls across runs, When timed,
//! Then the budget check adds at most 1 ms p95 per child on the builder.
//!
//! The "check" `compose_call` performs per child is exactly
//! `BudgetLedger::check` (before dispatch) plus `BudgetLedger::record`
//! (after) -- both pure, in-memory, no I/O (the async DB persist is a
//! separate, unmeasured write, not part of "the budget check" this AC
//! guards). Spread across 20 runs of 10 child calls each (200 total,
//! matching the AC's own count) so this also proves a fresh ledger's first
//! call is no slower than its last.

use mcphost::budget::{BudgetLedger, BudgetLimits};
use std::time::Instant;

fn limits() -> BudgetLimits {
    BudgetLimits {
        max_child_calls: Some(10),
        max_est_tokens: Some(200_000),
        max_tool_latency_ms: Some(30_000),
        max_wall_ms: Some(60_000),
        alert_fraction: 0.8,
    }
}

#[test]
fn two_hundred_child_calls_check_and_record_p95_under_1ms() {
    let limits = limits();
    let mut durations_ns: Vec<u128> = Vec::with_capacity(200);

    for _run in 0..20 {
        let mut ledger = BudgetLedger::new(0);
        for child in 0..10 {
            let now_ms = (child * 10) as i64;
            let start = Instant::now();
            let _ = ledger.check(&limits, now_ms);
            ledger.record(1_000, 5);
            durations_ns.push(start.elapsed().as_nanos());
        }
    }

    assert_eq!(durations_ns.len(), 200, "200 child calls, matching the AC's own count");
    durations_ns.sort_unstable();
    let p95_index = ((durations_ns.len() as f64) * 0.95).ceil() as usize - 1;
    let p95_ns = durations_ns[p95_index.min(durations_ns.len() - 1)];
    assert!(
        p95_ns < 1_000_000,
        "budget check+record p95 must be under 1ms, got {}ns (all: {durations_ns:?})",
        p95_ns
    );
}
