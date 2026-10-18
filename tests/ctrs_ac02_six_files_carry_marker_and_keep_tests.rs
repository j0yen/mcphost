//! PRD-mcphost-contract-suite-generated AC2 -- the six pre-existing contract
//! tests carry `//! contract:` and `cargo test --test suite_contracts_<part>_01
//! -- --list` lists every test fn their source declares, unchanged.

use std::fs;
use std::path::PathBuf;
use std::process::Command;

const SIX: [(&str, &str); 6] = [
    ("chg_ac01_changelog_headings_parse", "core"),
    ("ci_sandbox_ac06_sandbox_suites_run_as_a_parallel_job", "core"),
    ("cishard_ac01_balanced_shards_from_recorded_weights", "core"),
    ("compat_ac04_ac05_admin_and_tenant_cache_fields", "core"),
    ("lanecov_ac04_existing_lanes_unchanged", "sandbox"),
    ("toolname_ac08_tools_list_counts_updated", "core"),
];

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// Names of the `#[test]` / `#[tokio::test]` fns declared in a source file.
fn declared_tests(src: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut pending = false;
    for line in src.lines() {
        let t = line.trim();
        if t.starts_with("#[test]") || t.starts_with("#[tokio::test") {
            pending = true;
        } else if pending {
            let rest = t.strip_prefix("pub ").unwrap_or(t);
            let rest = rest.strip_prefix("async ").unwrap_or(rest);
            if let Some(r) = rest.strip_prefix("fn ") {
                let name: String = r.chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect();
                out.push(name);
                pending = false;
            } else if !t.starts_with("#[") && !t.starts_with("//") {
                pending = false;
            }
        }
    }
    out
}

fn list(suite: &str) -> String {
    let mut cmd = Command::new(env!("CARGO"));
    cmd.args(["test", "--test", suite, "--", "--list"]).current_dir(root());
    crate::common::strip_cargo_injected_env(&mut cmd);
    let out = cmd.output().expect("run cargo test -- --list");
    assert!(out.status.success(), "{suite} --list failed: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).into_owned()
}

#[test]
fn ctrs_ac02_each_file_carries_the_marker() {
    for (stem, _) in SIX {
        let src = fs::read_to_string(root().join(format!("tests/{stem}.rs"))).unwrap();
        assert!(
            src.lines().take(10).any(|l| l.starts_with("//! contract:")),
            "{stem}.rs lacks a `//! contract:` line in its first 10 lines"
        );
    }
}

#[test]
fn ctrs_ac02_contract_suites_list_every_test_of_the_six_files() {
    let core = list("suite_contracts_core_01");
    let sandbox = list("suite_contracts_sandbox_01");
    let mut total = 0;
    for (stem, part) in SIX {
        let src = fs::read_to_string(root().join(format!("tests/{stem}.rs"))).unwrap();
        let names = declared_tests(&src);
        assert!(!names.is_empty(), "{stem}.rs declares no tests?");
        let listing = if part == "core" { &core } else { &sandbox };
        for n in names {
            let want = format!("{stem}::{n}: test");
            assert!(
                listing.lines().any(|l| l == want),
                "suite_contracts_{part}_01 --list is missing `{want}`"
            );
            total += 1;
        }
    }
    assert!(total >= 12, "expected the six files to hold at least 12 tests, saw {total}");
}
