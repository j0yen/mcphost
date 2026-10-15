//! PRD-mcphost-contract-suite-generated AC6 -- on a warm cargo target
//! `scripts/pin-tests.sh` finishes every contract suite within 300 s total and
//! prints a per-binary timing line for each.
//!
//! The target is warm by construction: the test binary that runs this already
//! built the crate, and `pin-tests.sh` builds the two contract binaries once
//! up front outside the timed section (a cold link is not what the budget
//! measures).

use std::path::PathBuf;
use std::process::Command;
use std::time::Instant;

const BUDGET_SECS: u64 = 300;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

#[test]
fn ctrs_ac06_pin_tests_runs_all_contract_suites_within_300s_and_prints_timings() {
    let list = Command::new(root().join("scripts/pin-tests.sh"))
        .arg("--list")
        .current_dir(root())
        .output()
        .unwrap();
    let bins: Vec<String> = String::from_utf8_lossy(&list.stdout).lines().map(str::to_string).collect();
    assert!(!bins.is_empty());

    // warm the target (compile only), untimed
    for b in &bins {
        let st = Command::new(env!("CARGO"))
            .args(["test", "-q", "--no-run", "--test", b])
            .current_dir(root())
            .status()
            .unwrap();
        assert!(st.success(), "could not build {b}");
    }

    let start = Instant::now();
    let out = Command::new(root().join("scripts/pin-tests.sh"))
        .current_dir(root())
        .output()
        .unwrap();
    let elapsed = start.elapsed().as_secs();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "pin-tests.sh failed:\n{stdout}");
    assert!(elapsed <= BUDGET_SECS, "contract suites took {elapsed}s, budget {BUDGET_SECS}s:\n{stdout}");
    for b in &bins {
        let prefix = format!("pin-tests: pass {b} (");
        let line = stdout.lines().find(|l| l.starts_with(&prefix)).unwrap_or_else(|| panic!("no timing line for {b}:\n{stdout}"));
        assert!(line.ends_with("s)"), "{line}");
    }
    assert!(stdout.contains("pin-tests: ok ("), "{stdout}");
}
