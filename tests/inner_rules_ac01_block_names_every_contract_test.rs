//! contract: reads scripts/contract-lint.sh
//! PRD-less hand build: scripts/inner-rules.sh -- the generated coding-rules
//! block wm-build injects into every inner-coder prompt.
//! AC1 (rewritten by PRD-mcphost-contract-suite-registry-axis-b AC7) -- Given the
//! script, When it runs, Then its contract section prints one line per
//! `scripts/contract-lint.sh --definers` entry, the script source has no hand
//! array, and the tail names `scripts/pin-tests.sh`.

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
    let definers = run("scripts/contract-lint.sh", &["--definers", "tests"]);
    assert!(definers.lines().count() > 0, "--definers printed nothing");
    for l in definers.lines() {
        let entry = l.split('\t').next().unwrap();
        assert!(block.lines().any(|b| b.starts_with(&format!("{entry} \u{2014} "))), "block lacks a line for {entry}");
    }

    let src = fs::read_to_string(crate_root().join("scripts/inner-rules.sh")).unwrap();
    assert!(!src.contains(&format!("{}=(", "CONTRACT_TESTS")), "inner-rules.sh still carries a hand array");

    let tail = block.split("Done checks the daemon runs:").nth(1).expect("done-checks tail");
    assert!(tail.contains("scripts/pin-tests.sh"), "tail must name scripts/pin-tests.sh: {tail}");
}
