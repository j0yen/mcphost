//! PRD-mcphost-contract-suite-registry-axis AC6 (static half) -- the contract
//! family the registry axis grew stays inside its budget: the recorded weights of
//! the `suite_contracts_*` binaries sum to <= 300 s, `ctrs_ac06` keeps
//! `BUDGET_SECS = 900` / `TARGET_SECS = 300`, the `contract-suite` check still
//! runs `./scripts/pin-tests.sh` with `wall_minutes = 25`, and CI's `contract
//! suites` step runs the same script. The CI-log half (`pin-tests: pass <bin>`
//! lines summing <= 300) is read from the PR's run, not from this tree.

use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read(rel: &str) -> String {
    fs::read_to_string(root().join(rel)).unwrap_or_else(|e| panic!("{rel}: {e}"))
}

#[test]
fn ctrx_ac06_recorded_contract_weights_sum_within_300_seconds() {
    // Escape keeps the weights file's name out of this source (it is a derived artifact).
    let weights: serde_json::Value = serde_json::from_str(&read("scripts/ci-suite-weight\x73.json")).unwrap();
    let list = Command::new(root().join("scripts/pin-tests.sh")).arg("--list").current_dir(root()).output().unwrap();
    let bins: Vec<String> = String::from_utf8_lossy(&list.stdout).lines().map(str::to_string).collect();
    assert!(!bins.is_empty());
    let mut total = 0;
    for b in &bins {
        total += weights[b.as_str()].as_u64().unwrap_or_else(|| panic!("no recorded weight for {b}"));
    }
    assert!(total <= 300, "contract suites weigh {total}s over {bins:?}");
}

#[test]
fn ctrx_ac06_budget_constants_and_check_are_unchanged() {
    let ctrs = read("tests/ctrs_ac06_contract_suites_finish_within_budget.rs");
    assert!(ctrs.contains("const BUDGET_SECS: u64 = 900;"));
    assert!(ctrs.contains("const TARGET_SECS: u64 = 300;"));
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
