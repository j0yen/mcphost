//! PRD-mcphost-timing-assertion-lint
//! AC6 (P0) — Given the 4 former `ALLOW` files and the budget sites named in
//! Technical considerations, When `cargo test` runs each converted file with
//! and without `MCPHOST_PERF_SKIP=1`, Then every test passes and no budget
//! number changed.
//!
//! The with/without-skip runs are the suite itself (the gate runs it both
//! ways); this file pins the second half -- each converted file still spells
//! its original millisecond literal, now as the argument of a sanctioned
//! helper, so a later edit that quietly loosens a budget shows up here.

use std::fs;
use std::path::PathBuf;

/// (file, helper-call needles). The literal is the pre-conversion budget in
/// milliseconds (`Duration::from_secs(5)` -> 5000, `as_secs_f64() < 1.0` ->
/// 1000, ...).
const CONVERTED: &[(&str, &[&str])] = &[
    // the 4 former ALLOW files
    ("checkcompat_race_ac01_previous_up_within_1s.rs", &["deadline_budget!(1000,"]),
    ("python_ac01_no_deps_cpu_memory.rs", &["deadline_budget!(5000,"]),
    ("ac15_call_timeout.rs", &["deadline_budget!(5000,"]),
    (
        "tablemodel_ac06_large_table_samples_10000_under_500ms.rs",
        &["deadline_budget!(500,"],
    ),
    // budget sites named in Technical considerations
    ("drift_ac09_fifty_queries_rerun_under_5s_one_call.rs", &["deadline_budget!(5000,"]),
    ("hooks_ac1_hmac_sha256_valid_signature_runs.rs", &["deadline_budget!(500,"]),
    ("oauthconf_ac01_gate_verdict_table_matches_gold.rs", &["deadline_budget!(20_000,"]),
    (
        "wake_ac6_msg_wait.rs",
        &["deadline_budget!(3000,", "deadline_budget!(4000, at_least = 1900,"],
    ),
    (
        "xlinks_ac07_ci_docs_job_fails_on_a_404_and_stays_fast.rs",
        &["deadline_budget!(60_000,"],
    ),
    (
        "sqlpass_ac04_group_by_logs_row_count_and_duration.rs",
        &["perf_overhead_budget!("],
    ),
];

#[test]
fn every_converted_file_spells_its_original_budget_through_a_helper() {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests");
    let mut problems = Vec::new();
    for (file, needles) in CONVERTED {
        let src = fs::read_to_string(dir.join(file)).unwrap_or_else(|e| panic!("read {file}: {e}"));
        for needle in *needles {
            if !src.contains(needle) {
                problems.push(format!("{file}: missing `{needle}`"));
            }
        }
    }
    assert!(problems.is_empty(), "budget sites changed or unwrapped:\n{}", problems.join("\n"));
}
