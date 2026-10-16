//! PRD-mcphost-contract-suite-markers AC6 -- on the committed tree
//! `scripts/contract-lint.sh tests` exits 0 and prints the count of marked
//! files, and `scripts/flake-lint.sh` still invokes it.

use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

#[test]
fn ctrm_ac06_committed_tree_is_clean_and_reports_the_marked_count() {
    let out = Command::new(root().join("scripts/contract-lint.sh"))
        .arg("tests")
        .current_dir(root())
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    assert_eq!(out.status.code(), Some(0), "{stdout}");
    let count: usize = stdout
        .split("(")
        .nth(1)
        .and_then(|rest| rest.split_whitespace().next())
        .and_then(|n| n.parse().ok())
        .unwrap_or_else(|| panic!("no marked count in {stdout}"));
    let on_disk = fs::read_dir(root().join("tests"))
        .unwrap()
        .filter_map(Result::ok)
        .filter(|e| e.path().extension().is_some_and(|x| x == "rs"))
        .filter(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            !["suite_core_", "suite_sandbox_", "suite_env_", "suite_contracts_"].iter().any(|p| name.starts_with(p))
        })
        .filter(|e| fs::read_to_string(e.path()).unwrap().lines().take(10).any(|l| l.starts_with("//! contract:")))
        .count();
    assert_eq!(count, on_disk, "{stdout}");
    assert!(count >= 6, "{stdout}");
}

#[test]
fn ctrm_ac06_flake_lint_still_runs_the_contract_lint() {
    let script = fs::read_to_string(root().join("scripts/flake-lint.sh")).unwrap();
    assert!(script.matches("contract-lint.sh").count() >= 1);
}
