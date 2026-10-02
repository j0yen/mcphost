//! PRD-mcphost-test-suite-flake-lints
//! AC7 (P0) — Given the repository at the landing commit, When
//! `flake-lint.sh` runs over `tests/`, Then exit 0 with at most 5
//! allow-listed entries, and `gen-test-suites.sh` has placed every
//! `EnvGuard` test in a `suite_env_*` binary.
//!
//! Runs against the real, already-committed repo tree (not a scratch copy)
//! -- this is a property of the landed state itself, same convention
//! `tests/suite_ac1_ten_binaries_and_names_preserved.rs` uses for
//! `gen-test-suites.sh`'s own suite-count invariant.

use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

#[test]
fn real_repo_lint_passes_with_at_most_five_allow_listed_entries() {
    let root = repo_root();
    let out = Command::new(root.join("scripts/flake-lint.sh"))
        .arg("tests")
        .current_dir(&root)
        .output()
        .expect("run scripts/flake-lint.sh tests");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "flake-lint.sh must exit 0 over the real tests/ tree at the landing \
         commit; stdout:\n{stdout}\nstderr: {}",
        String::from_utf8_lossy(&out.stderr),
    );

    let allow_file = root.join("tests/flake-lint-allow.txt");
    let entries = if allow_file.exists() {
        fs::read_to_string(&allow_file)
            .expect("read tests/flake-lint-allow.txt")
            .lines()
            .map(|line| line.split('#').next().unwrap_or("").trim())
            .filter(|line| !line.is_empty())
            .count()
    } else {
        0
    };
    assert!(
        entries <= 5,
        "the allow-list may hold at most 5 entries at landing; found {entries} in \
         tests/flake-lint-allow.txt"
    );
}

/// "Contains EnvGuard" is the same textual, conservative test
/// `scripts/gen-test-suites.sh`'s own `is_env_guarded` uses to route a file
/// into `suite_env_*` -- this test re-derives the same membership
/// independently (reading `tests/*.rs` source text directly) rather than
/// importing the generator's python, so a regression in either one shows up
/// as a disagreement instead of both staying silently wrong together.
#[test]
fn every_env_guard_test_is_in_a_suite_env_binary() {
    let root = repo_root();
    let tests_dir = root.join("tests");

    let suite_env_content: String = fs::read_dir(&tests_dir)
        .expect("read tests/")
        .flatten()
        .filter(|entry| {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            name.starts_with("suite_env_") && name.ends_with(".rs")
        })
        .map(|entry| fs::read_to_string(entry.path()).unwrap_or_default())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        !suite_env_content.is_empty(),
        "no tests/suite_env_*.rs files found -- has scripts/gen-test-suites.sh been run?"
    );

    let mut missing = Vec::new();
    for entry in fs::read_dir(&tests_dir).expect("read tests/").flatten() {
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("rs") {
            continue;
        }
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        if name.starts_with("suite_") {
            continue; // a generated suite binary itself, not a member file
        }
        let content = fs::read_to_string(&path).unwrap_or_default();
        if !(content.contains("EnvGuard") || content.contains("AdvisoryModeGuard")) {
            continue;
        }
        let marker = format!("#[path = \"{name}\"]");
        if !suite_env_content.contains(&marker) {
            missing.push(name);
        }
    }

    assert!(
        missing.is_empty(),
        "these EnvGuard-using tests/*.rs files are not included in any \
         tests/suite_env_*.rs binary: {missing:?}"
    );
}
