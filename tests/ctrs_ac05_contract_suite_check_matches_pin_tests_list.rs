//! PRD-mcphost-contract-suite-generated AC5 -- `wm-build-ci.toml` declares a
//! `[[check]]` named `contract-suite` whose command is `scripts/pin-tests.sh`,
//! and that command's `--list` output names the same binaries as the
//! contract `[[test]]` entries in Cargo.toml (one source, no drift).

use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// The `cmd` of the `[[check]]` block with the given name.
fn check_cmd(toml: &str, name: &str) -> Option<String> {
    let mut in_check = false;
    let mut matched = false;
    let mut cmd = None;
    for line in toml.lines().map(str::trim) {
        if line == "[[check]]" {
            in_check = true;
            matched = false;
            cmd = None;
        } else if line.starts_with('[') {
            in_check = false;
        } else if in_check {
            if line == format!("name = \"{name}\"") {
                matched = true;
            } else if let Some(v) = line.strip_prefix("cmd = \"") {
                cmd = Some(v.trim_end_matches('"').to_string());
            }
            if matched && let Some(c) = &cmd {
                return Some(c.clone());
            }
        }
    }
    None
}

#[test]
fn ctrs_ac05_contract_suite_check_runs_pin_tests_and_lists_the_cargo_toml_binaries() {
    let toml = fs::read_to_string(root().join("wm-build-ci.toml")).unwrap();
    let cmd = check_cmd(&toml, "contract-suite").expect("wm-build-ci.toml declares [[check]] contract-suite");
    assert_eq!(cmd.trim_start_matches("./"), "scripts/pin-tests.sh", "command is exactly the script, cargo form");

    let out = Command::new(root().join(cmd.trim_start_matches("./")))
        .arg("--list")
        .current_dir(root())
        .output()
        .unwrap();
    assert!(out.status.success());
    let mut listed: Vec<String> = String::from_utf8_lossy(&out.stdout).lines().map(str::to_string).collect();
    listed.sort();

    let cargo = fs::read_to_string(root().join("Cargo.toml")).unwrap();
    let mut declared: Vec<String> = cargo
        .lines()
        .filter_map(|l| l.strip_prefix("name = \""))
        .map(|l| l.trim_end_matches('"').to_string())
        .filter(|n| n.starts_with("suite_contracts_"))
        .collect();
    declared.sort();
    assert!(!declared.is_empty());
    assert_eq!(listed, declared);

    // the cargo-free gate check stays alongside it
    assert_eq!(check_cmd(&toml, "pin-tests").as_deref(), Some("./scripts/pin-tests.sh --cargo-free"));
}
