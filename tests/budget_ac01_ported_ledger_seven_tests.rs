//! PRD-mcphost-run-budget-governor
//! AC1 (P0) — Given the ported ledger, When the seven carried-over unit
//! tests run, Then they pass, including `Alert` at 0.8 and `Exceeded` at
//! 1.0 on each dimension.
//!
//! Ported from ai-stack's `mcp-query-budget-governor` (`~/projects/ai-stack`
//! @ 4ef6c22): one `Ok`-under-every-limit test, one generic `Alert`-at-0.8
//! test, one `Exceeded`-at-1.0 test per dimension (four), and one
//! `fraction_used` test -- seven total, matching the source crate's own
//! count (requirement 1: "The crate's seven tests are carried over").

use mcphost::budget::{BudgetLedger, BudgetLimits, Dimension, Verdict};

fn limits() -> BudgetLimits {
    BudgetLimits {
        max_child_calls: Some(10),
        max_est_tokens: Some(1_000),
        max_tool_latency_ms: Some(5_000),
        max_wall_ms: Some(60_000),
        alert_fraction: 0.8,
    }
}

#[test]
fn ok_when_every_dimension_is_well_under_its_limit() {
    let mut ledger = BudgetLedger::new(0);
    ledger.record(10, 100);
    assert_eq!(ledger.check(&limits(), 1_000), Verdict::Ok);
}

#[test]
fn alert_fires_at_0_8_fraction_on_any_dimension() {
    // 8 of 10 child calls -- exactly the 0.8 alert threshold.
    let mut ledger = BudgetLedger::new(0);
    for _ in 0..8 {
        ledger.record(1, 1);
    }
    assert_eq!(ledger.check(&limits(), 1_000), Verdict::Alert);
}

#[test]
fn exceeded_child_calls_at_1_0_fraction() {
    let mut ledger = BudgetLedger::new(0);
    for _ in 0..10 {
        ledger.record(1, 1);
    }
    assert_eq!(
        ledger.check(&limits(), 1_000),
        Verdict::Exceeded { dimension: Dimension::ChildCalls }
    );
}

#[test]
fn exceeded_est_tokens_at_1_0_fraction() {
    let mut ledger = BudgetLedger::new(0);
    ledger.record(1_000, 1);
    assert_eq!(
        ledger.check(&limits(), 1_000),
        Verdict::Exceeded { dimension: Dimension::EstTokens }
    );
}

#[test]
fn exceeded_tool_latency_ms_at_1_0_fraction() {
    let mut ledger = BudgetLedger::new(0);
    ledger.record(1, 5_000);
    assert_eq!(
        ledger.check(&limits(), 1_000),
        Verdict::Exceeded { dimension: Dimension::ToolLatencyMs }
    );
}

#[test]
fn exceeded_wall_ms_at_1_0_fraction() {
    let ledger = BudgetLedger::new(0);
    // No records at all -- wall clock alone crosses the limit.
    assert_eq!(
        ledger.check(&limits(), 60_000),
        Verdict::Exceeded { dimension: Dimension::WallMs }
    );
}

#[test]
fn fraction_used_is_the_max_across_active_dimensions() {
    let mut ledger = BudgetLedger::new(0);
    // 3/10 child calls (0.3), 900/1000 est_tokens (0.9) -- the max must win.
    for _ in 0..3 {
        ledger.record(300, 1);
    }
    let fraction = ledger.fraction_used(&limits(), 1_000);
    assert!(
        (fraction - 0.9).abs() < 1e-9,
        "expected max fraction 0.9 (est_tokens), got {fraction}"
    );
}
