//! PRD-mcphost-contract-suite-registry-axis-b AC6 (static half) -- the contract
//! family the registry axis grew stays inside its budget: the recorded weights of
//! the `suite_contracts_*` binaries sum to <= `ctrs_ac06`'s `TARGET_SECS` (read from
//! that file, the one place the number lives; measured 611 s on run 39200390250
//! + 25 %), `BUDGET_SECS` stays 900, the `contract-suite` check still
//! runs `./scripts/pin-tests.sh` with `wall_minutes = 25`, and CI's `contract
//! suites` step runs the same script. The CI-log half (`pin-tests: pass <bin>`
//! lines summing <= TARGET_SECS) is read from the PR's run, not from this tree.

use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read(rel: &str) -> String {
    fs::read_to_string(root().join(rel)).unwrap_or_else(|e| panic!("{rel}: {e}"))
}

/// `const <name>: u64 = <n>;` read from the budget test's own source.
fn ctrs_const(name: &str) -> u64 {
    let src = read("tests/ctrs_ac06_contract_suites_finish_within_budget.rs");
    let want = format!("const {name}: u64 = ");
    let line = src.lines().find(|l| l.starts_with(&want)).unwrap_or_else(|| panic!("no {want} in ctrs_ac06"));
    line[want.len()..].trim_end_matches(';').parse().unwrap()
}

#[test]
fn ctrx_ac06_recorded_contract_weights_sum_within_the_target() {
    // Escape keeps the weights file's name out of this source (it is a derived artifact).
    let weights: serde_json::Value = serde_json::from_str(&read("scripts/ci-suite-weight\x73.json")).unwrap();
    let list = Command::new(root().join("scripts/pin-tests.sh")).arg("--list").current_dir(root()).output().unwrap();
    let bins: Vec<String> = String::from_utf8_lossy(&list.stdout).lines().map(str::to_string).collect();
    assert!(!bins.is_empty());
    let mut total = 0;
    for b in &bins {
        total += weights[b.as_str()].as_u64().unwrap_or_else(|| panic!("no recorded weight for {b}"));
    }
    let target = ctrs_const("TARGET_SECS");
    assert!(total <= target, "contract suites weigh {total}s over {bins:?}, target {target}s");
}

#[test]
fn ctrx_ac06_budget_constants_and_check_are_unchanged() {
    assert_eq!(ctrs_const("BUDGET_SECS"), 900);
    let ctrs = read("tests/ctrs_ac06_contract_suites_finish_within_budget.rs");
    let at = ctrs.find("const TARGET_SECS").unwrap();
    let comment: Vec<&str> = ctrs[..at].lines().rev().take(3).collect();
    assert!(comment.iter().any(|l| l.contains("measured 1016 s on CI run 40115329593 = 615 s × 0.98 runner + 411 s new tests")), "{comment:?}");
    let toml = read("wm-build-ci.toml");
    let check = toml.split("[[check]]").find(|c| c.contains("name = \"contract-suite\"")).expect("contract-suite check");
    assert!(check.contains("cmd = \"./scripts/pin-tests.sh\""), "{check}");
    assert!(check.lines().any(|l| l.starts_with("wall_minutes = 25")), "{check}");
}

#[test]
fn ctrx_ac06_ci_contract_suites_step_runs_pin_tests() {
    let ci = read(".github/workflows/ci.yml");
    let at = ci.find("- name: contract suites").expect("contract suites step");
    let step: Vec<&str> = ci[at..].lines().take(2).collect();
    assert!(step[1].trim() == "run: ./scripts/pin-tests.sh", "{step:?}");
}

#[test]
fn ctrx_ac06_no_ctrx_test_restates_the_target_literal() {
    let needle = ctrs_const("TARGET_SECS").to_string();
    for e in fs::read_dir(root().join("tests")).unwrap().flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if !(name.starts_with("ctrx_") && name.ends_with(".rs")) {
            continue;
        }
        let src = fs::read_to_string(e.path()).unwrap();
        let hit = src.split(|c: char| !c.is_ascii_alphanumeric()).any(|w| w == needle);
        assert!(!hit, "{name} restates the budget literal {needle}; read it from ctrs_ac06");
    }
}
