//! PRD-mcphost-performance-guarantees-measured AC2 (drift half): the plan's
//! `perf_*_p95_ms` fields and `perf::PATHS` are the same set.

use mcphost::perf::{PATHS, path_of_field};
use mcphost::plans::PlanCatalog;
use std::collections::BTreeSet;

/// Panics when `perf::PATHS` and the `perf_*_p95_ms` field suffixes of any
/// plan's `ceilings_json()` differ (either direction).
pub fn assert_paths_match_plan_fields() {
    let paths: BTreeSet<String> = PATHS.iter().map(|(n, _)| n.to_string()).collect();
    let catalog = PlanCatalog::default_catalog();
    for plan in ["free", "pro"] {
        let ceilings = catalog.get(plan).expect("plan").ceilings_json();
        let fields: BTreeSet<String> = ceilings
            .as_object()
            .expect("object")
            .keys()
            .filter_map(|k| path_of_field(k).map(str::to_string))
            .collect();
        assert_eq!(fields, paths, "plan {plan}: perf_*_p95_ms field suffixes differ from perf::PATHS");
    }
}

#[test]
fn perf_paths_and_plan_field_suffixes_are_the_same_set() {
    assert_paths_match_plan_fields();
}
