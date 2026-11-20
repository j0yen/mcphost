//! PRD-mcphost-performance-guarantees-measured
//! AC3 (P0) -- Given the hourly task with a 2 s test interval, When two
//! cycles pass in a test, Then `perf_samples` holds 2 rows per (plan, path)
//! with `n == 20` and `p95_ms >= p50_ms`.

use crate::common;
use mcphost::perf::{self, ProbeFuture};
use mcphost::state::AppState;
use std::time::Duration;

fn quick<'a>(_s: &'a AppState, _t: i64, _ns: &'a str) -> ProbeFuture<'a> {
    Box::pin(async {
        tokio::time::sleep(Duration::from_millis(1)).await;
        true
    })
}

#[tokio::test]
async fn pgm_ac03_two_cycles_leave_two_rows_per_plan_and_path() {
    let (state, _dir) = common::bare_app_state().await;
    let probes: perf::Probes = vec![("echo", quick), ("python_cold", quick), ("http", quick)];
    let handle = perf::spawn_tick_for_test(state.clone(), Duration::from_secs(2), probes);
    // Expected keys come from the plan/path definitions, never from the
    // rows table. Poll (not a fixed sleep) so a loaded host that stretches a
    // cycle past 2 s still passes; a missing cycle fails at the deadline.
    let keys: Vec<(String, String)> = perf::PLANS
        .iter()
        .flat_map(|plan| perf::PATHS.iter().map(move |(path, _)| (plan.to_string(), path.to_string())))
        .collect();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
    loop {
        let mut counts = Vec::new();
        for (plan, path) in &keys {
            let rows = state.db.recent_perf_samples(plan.clone(), path.clone(), 10).await.expect("rows");
            counts.push((plan.clone(), path.clone(), rows.len()));
        }
        if counts.iter().all(|(_, _, n)| *n >= 2) {
            break;
        }
        assert!(tokio::time::Instant::now() < deadline, "deadline: rows per (plan, path): {counts:?}");
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    handle.abort();

    for (plan, path) in &keys {
        let rows = state.db.recent_perf_samples(plan.clone(), path.clone(), 10).await.expect("rows");
        assert!(rows.len() >= 2, "{plan}/{path}: {rows:?}");
        // No cycle-id column: two cycles are told apart by distinct `ts`.
        assert_ne!(rows[0].ts, rows[1].ts, "{plan}/{path}: {rows:?}");
        for r in rows {
            assert_eq!(r.n, 20, "{plan}/{path}");
            assert!(r.p95_ms >= r.p50_ms, "{plan}/{path}: {r:?}");
        }
    }
}
