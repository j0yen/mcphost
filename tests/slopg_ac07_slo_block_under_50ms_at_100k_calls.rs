//! PRD-mcphost-slo-targets-on-status AC7 (P1) — Given a planted DB with
//! 100 000 `calls` rows, When the `slo` block is computed, Then it completes
//! in ≤ 50 ms wall time in the test. The 24 h numbers are one range scan over
//! `idx_calls_started_unix`, not a full scan: the rows are planted across the
//! last 7 days (the retention a free plan keeps), so a scan that reads the
//! table instead of the window reads about seven times the rows it needs.

use crate::common::{TestServer, signup};
use mcphost::statusfeed::{SLO_TARGETS, host_fault_codes, slo_block};
use serde_json::Value;

const ROWS: i64 = 100_000;
const SPREAD_SECS: i64 = 7 * 86_400;

/// How many planted rows `Db::insert_slo_calls_bulk_for_test` puts at or after
/// `window_start` (row `i` is at `end - 1 - i * SPREAD_SECS / ROWS`).
fn rows_since(end: i64, window_start: i64) -> i64 {
    (0..ROWS).filter(|i| end - 1 - (i * SPREAD_SECS) / ROWS >= window_start).count() as i64
}

#[tokio::test]
async fn slopg_ac07_slo_block_over_100k_calls_rows_under_50ms() {
    let server = TestServer::start().await;
    let (ns, _key) = signup(&server.base_url, "Slo Scale").await;
    let tenant = server.state.db.find_tenant_by_namespace(ns).await.expect("find").expect("tenant");
    let end = mcphost::state::now_unix();
    server
        .state
        .db
        .insert_slo_calls_bulk_for_test(tenant.id, ROWS, end, SPREAD_SECS, host_fault_codes()[0].to_string())
        .await
        .expect("plant 100 000 calls rows");

    // The block measured the planted rows of the last 24 h and no others.
    let block = slo_block(&server.state).await.expect("slo block");
    let after = mcphost::state::now_unix();
    let rate = &block["error_rate_24h"];
    let n = rate["sample_n"].as_i64().expect("sample_n");
    let (lo, hi) = (rows_since(end, after - 86_400), rows_since(end, end - 86_400));
    assert!((lo..=hi).contains(&n) && n < ROWS / 2, "sample_n {n} outside the planted 24 h rows {lo}..={hi}: {rate}");
    assert!(rate["measured"].as_f64().is_some_and(|m| (1.9..=2.1).contains(&m)), "every 50th row is a host fault: {rate}");
    assert!(block["tool_call_p95_ms_24h"]["measured"].is_number());
    assert!(block.as_object().is_some_and(|o| o.len() == SLO_TARGETS.len()));

    // The window is a covering index range scan, so the plan names the index.
    let plan = server.state.db.explain_slo_window_for_test().await.expect("explain");
    assert!(plan.contains("COVERING INDEX idx_calls_started_unix"), "the 24 h scan must use the started_unix index: {plan}");

    // Median-of-5 warm budget; skipped under MCPHOST_PERF_SKIP=1 (loaded host).
    crate::perf_budget!(50, {
        let v: Value = slo_block(&server.state).await.expect("slo block");
        assert!(v["error_rate_24h"]["measured"].is_number());
    });
}
