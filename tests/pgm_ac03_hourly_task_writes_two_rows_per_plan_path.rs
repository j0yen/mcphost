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
    // Cycles fire at ~2 s and ~4 s; read between the second and third.
    tokio::time::sleep(Duration::from_millis(5_000)).await;
    handle.abort();

    for plan in perf::PLANS {
        for (path, _) in perf::PATHS {
            let rows = state
                .db
                .recent_perf_samples(plan.to_string(), path.to_string(), 10)
                .await
                .expect("rows");
            assert_eq!(rows.len(), 2, "{plan}/{path}: {rows:?}");
            for r in rows {
                assert_eq!(r.n, 20, "{plan}/{path}");
                assert!(r.p95_ms >= r.p50_ms, "{plan}/{path}: {r:?}");
            }
        }
    }
}
