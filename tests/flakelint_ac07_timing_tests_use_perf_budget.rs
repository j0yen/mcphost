//! Flake lint: every `tests/*.rs` file that asserts on elapsed wall time with
//! a `<` comparison must route it through `perf_budget!` (which honors
//! `MCPHOST_PERF_SKIP=1` on a loaded host), otherwise the assertion fails
//! under load and the test gets quarantined as a baseline flake.

use std::path::Path;

const PATTERNS: &[&str] = &[
    ".as_millis() <",
    ".as_micros() <",
    ".as_secs_f64() <",
    "elapsed() <",
];

/// Files with a pre-existing timing comparison that this lint's PR did not
/// convert; each is a follow-up, not a license to add more.
const ALLOW: &[&str] = &[
    // already honors MCPHOST_PERF_SKIP by hand (no perf_budget! literal)
    "checkcompat_race_ac01_previous_up_within_1s.rs",
    // 5 s hang-guard deadlines, not performance budgets
    "python_ac01_no_deps_cpu_memory.rs",
    "ac15_call_timeout.rs",
    // single-shot 500 ms budget, still to be wrapped in perf_budget!
    "tablemodel_ac06_large_table_samples_10000_under_500ms.rs",
];

#[test]
fn every_timing_comparison_test_file_uses_perf_budget() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests");
    let mut offenders = Vec::new();
    for entry in std::fs::read_dir(&dir).expect("read tests/") {
        let path = entry.expect("dir entry").path();
        if path.extension().is_none_or(|e| e != "rs") {
            continue;
        }
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        if ALLOW.contains(&name.as_str()) {
            continue;
        }
        let src = std::fs::read_to_string(&path).expect("read test file");
        let hits: Vec<&&str> = PATTERNS.iter().filter(|p| src.contains(**p)).collect();
        if !hits.is_empty() && !src.contains("perf_budget!") {
            offenders.push(format!("{name}: {hits:?}"));
        }
    }
    offenders.sort();
    assert!(
        offenders.is_empty(),
        "timing assertions without perf_budget! (wrap in crate::perf_budget!(ms, {{ .. }})):\n{}",
        offenders.join("\n")
    );
}
