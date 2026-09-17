//! PRD-mcphost-proof-lane-loop-config, AC2 — the coverage test runs under
//! `cargo test` and passes.
//!
//! Deviation from AC2's literal wording, stated here rather than buried in
//! a journal line: AC2 was drafted as `cargo test --test
//! lanecov_ac01_every_tracked_path_routes`, i.e. it assumed every
//! `tests/*.rs` file is its own integration-test binary. This crate turned
//! that off in PRD-mcphost-test-suite-consolidation (`autotests = false` in
//! Cargo.toml plus generated `tests/suite_<core|sandbox>_NN.rs` files that
//! `#[path]`-include each test file by name), because 289 per-file binaries
//! cost ~80 GB of `target/debug/deps`. Under that convention no per-file
//! `--test <stem>` target exists for ANY test in this repo, and
//! `scripts/gen-test-suites.sh --check` actively refuses a hand-added
//! `[[test]]` entry that it would not itself generate -- so re-adding one
//! just to match AC2's literal spelling would red the gate it is meant to
//! keep green.
//!
//! What AC2 actually asks for -- "the coverage test runs, and passes" -- is
//! therefore proved two ways: the run itself
//! (`cargo test --test suite_core_06 lanecov`, or plain `cargo test
//! --workspace`, which is what CI and every proof lane invoke), and the
//! mechanical assertion below that no `lanecov_*` file can silently fall
//! out of a suite and stop being compiled at all. The second half is the
//! part a passing run cannot prove about itself: a test file that no suite
//! includes is not a failing test, it is no test.

use std::fs;
use std::path::Path;

fn manifest_dir() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

/// AC2: every `tests/lanecov_ac*.rs` proof is `#[path]`-included by some
/// generated `tests/suite_*.rs`, and that suite is a declared `[[test]]`
/// target in Cargo.toml -- so `cargo test` really does run them.
#[test]
fn lanecov_ac02_every_lanecov_file_is_a_declared_suite_member() {
    let dir = manifest_dir();
    let tests_dir = dir.join("tests");

    let mut lanecov_files: Vec<String> = fs::read_dir(&tests_dir)
        .expect("tests/ must be readable")
        .filter_map(|e| e.ok())
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| n.starts_with("lanecov_") && n.ends_with(".rs"))
        .collect();
    lanecov_files.sort();
    assert!(
        !lanecov_files.is_empty(),
        "no tests/lanecov_*.rs proofs found; this PRD's coverage test has vanished"
    );

    let cargo_toml = fs::read_to_string(dir.join("Cargo.toml")).expect("Cargo.toml must exist");

    // Collect the suites that are actually declared as [[test]] targets.
    let declared_suites: Vec<String> = cargo_toml
        .lines()
        .filter_map(|l| l.trim().strip_prefix("path = \"tests/"))
        .filter_map(|l| l.strip_suffix("\""))
        .map(|s| s.to_string())
        .collect();
    assert!(
        !declared_suites.is_empty(),
        "Cargo.toml declares no tests/*.rs [[test]] targets; gen-test-suites.sh output is missing"
    );

    // Index which declared suite includes which member file.
    let mut suite_bodies: Vec<(String, String)> = Vec::new();
    for suite in &declared_suites {
        let body = fs::read_to_string(tests_dir.join(suite))
            .unwrap_or_else(|e| panic!("declared [[test]] target tests/{suite} is unreadable: {e}"));
        suite_bodies.push((suite.clone(), body));
    }

    let mut orphans: Vec<String> = Vec::new();
    for file in &lanecov_files {
        let needle = format!("#[path = \"{file}\"]");
        if !suite_bodies.iter().any(|(_, body)| body.contains(&needle)) {
            orphans.push(file.clone());
        }
    }
    assert!(
        orphans.is_empty(),
        "these lanecov proof file(s) are included by no declared [[test]] suite, so `cargo test` \
         never compiles or runs them (run scripts/gen-test-suites.sh): {orphans:?}"
    );
}
