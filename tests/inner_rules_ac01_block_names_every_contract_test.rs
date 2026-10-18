//! PRD-less hand build: scripts/inner-rules.sh -- the generated coding-rules
//! block wm-build injects into every inner-coder prompt.
//! AC1 (rewritten by PRD-mcphost-contract-suite-registry-axis AC7) -- Given the
//! script, When it runs, Then every `scripts/contract-lint.sh --artifacts` entry
//! appears in the block, the script source has no hand array, and the block
//! mentions `flake-lint` and `scripts/pin-tests.sh`.

use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn crate_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn run(script: &str, args: &[&str]) -> String {
    let out = Command::new("bash")
        .arg(crate_root().join(script))
        .args(args)
        .current_dir(crate_root())
        .output()
        .unwrap_or_else(|e| panic!("run {script}: {e}"));
    assert!(out.status.success(), "{script} failed: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).into_owned()
}

#[test]
fn inner_rules_block_names_every_contract_test() {
    let block = run("scripts/inner-rules.sh", &[]);
    let artifacts = run("scripts/contract-lint.sh", &["--artifacts", "tests"]);
    assert!(artifacts.lines().count() > 0, "--artifacts printed nothing");
    for entry in artifacts.lines() {
        assert!(block.lines().any(|l| l.starts_with(&format!("{entry} \u{2014} "))), "block lacks a line for {entry}");
    }

    let src = fs::read_to_string(crate_root().join("scripts/inner-rules.sh")).unwrap();
    assert!(!src.contains(&format!("{}=(", "CONTRACT_TESTS")), "inner-rules.sh still carries a hand array");

    assert!(block.contains("flake-lint"), "block must mention flake-lint");
    let tail = block.split("Done checks the daemon runs:").nth(1).expect("done-checks tail");
    assert!(tail.contains("scripts/pin-tests.sh"), "tail must name scripts/pin-tests.sh: {tail}");
}
