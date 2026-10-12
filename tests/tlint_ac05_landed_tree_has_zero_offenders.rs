//! PRD-mcphost-timing-assertion-lint
//! AC5 (P0) — Given mcphost at this PRD's landing commit, When
//! `scripts/timing-lint.py tests` runs, Then exit 0 with `0 offenders`, and
//! `grep -c 'const ALLOW\|const PATTERNS'
//! tests/flakelint_ac07_timing_tests_use_perf_budget.rs` is 0 and
//! `tests/flake-lint-allow.txt` has no `single-shot-perf` entry.
//!
//! Runs against the real, already-committed repo tree (a property of the
//! landed state itself), like `flakelint_ac07`.

use std::path::PathBuf;
use std::process::Command;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

#[test]
fn real_tests_tree_has_zero_offenders_and_the_four_documented_helpers() {
    let root = repo_root();
    let out = Command::new("python3")
        .arg("-I")
        .arg(root.join("scripts/timing-lint.py"))
        .arg("tests")
        .current_dir(&root)
        .output()
        .expect("run scripts/timing-lint.py tests");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(0), "stdout:\n{stdout}");
    assert!(
        stdout.contains("timing-lint: 0 offenders, 4 helpers"),
        "the sanctioned set at landing is perf_budget, perf_overhead_budget, \
         deadline_budget, serve_ready_timeout; stdout:\n{stdout}"
    );
}

#[test]
fn hand_typed_lists_and_the_single_shot_allow_entry_are_gone() {
    let root = repo_root();
    let ac07 = std::fs::read_to_string(root.join("tests/flakelint_ac07_timing_tests_use_perf_budget.rs"))
        .expect("read flakelint_ac07");
    // Built from pieces so this file does not itself contain the needles.
    for needle in [["const", " ALLOW"].concat(), ["const", " PATTERNS"].concat()] {
        assert!(!ac07.contains(&needle), "flakelint_ac07 still declares `{needle}`");
    }
    let allow = std::fs::read_to_string(root.join("tests/flake-lint-allow.txt"))
        .expect("read flake-lint-allow.txt");
    let rule = ["single", "-shot-", "perf"].concat();
    let live: Vec<&str> = allow
        .lines()
        .map(|l| l.split('#').next().unwrap_or("").trim())
        .filter(|l| l.contains(&rule))
        .collect();
    assert!(live.is_empty(), "allow-list still has {rule} entries: {live:?}");
}
