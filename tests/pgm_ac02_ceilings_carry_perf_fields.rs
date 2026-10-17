//! PRD-mcphost-performance-guarantees-measured
//! AC2 (P0) -- Given the plan catalog, When `ceilings_json()` is called for
//! free and pro, Then it contains the three `perf_*` fields and
//! `tests/pgm_drift.rs` fails if `perf::PATHS` and the field suffixes differ.

use mcphost::plans::PlanCatalog;
use serde_json::json;

#[test]
fn pgm_ac02_ceilings_json_has_the_three_perf_fields_with_published_values() {
    let catalog = PlanCatalog::default_catalog();
    for (plan, echo, cold, http) in [("free", 150, 1500, 800), ("pro", 100, 800, 500)] {
        let c = catalog.get(plan).expect("plan").ceilings_json();
        assert_eq!(c["perf_echo_p95_ms"], json!(echo), "{plan}");
        assert_eq!(c["perf_python_cold_p95_ms"], json!(cold), "{plan}");
        assert_eq!(c["perf_http_p95_ms"], json!(http), "{plan}");
    }
}

#[test]
fn pgm_ac02_drift_test_agrees_with_paths() {
    crate::pgm_drift::assert_paths_match_plan_fields();
}
