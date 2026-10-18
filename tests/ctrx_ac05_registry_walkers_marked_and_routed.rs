//! PRD-mcphost-contract-suite-registry-axis-b AC5 -- on the committed tree the
//! contract lint (plain and `--fix`), `gen-test-suites.sh --check` and
//! `ci-test-partition.sh check` all exit 0 with no `add` line; the three gap files
//! and `ctrm_ac05` carry a marker, each in one `suite_contracts_<core|sandbox>_NN.rs`;
//! `verbfwd_ac08` declares only the descriptor registry, `aliastruth_ac01` defines
//! the alias table, `ctrs_ac02`/`ctrs_ac06` stay unmarked; no test file of the
//! Cited-tree's suites is lost.

use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};

const CITED: &str = "9f0bd02a29b62c2c14263854815bb4898b81ed64";

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn run(script: &str, args: &[&str]) -> Output {
    Command::new(root().join("scripts").join(script)).args(args).current_dir(root()).output().unwrap()
}

fn text(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned() + &String::from_utf8_lossy(&o.stderr)
}

fn contract_suites() -> Vec<(String, String)> {
    let mut out = Vec::new();
    for e in fs::read_dir(root().join("tests")).unwrap().flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if name.starts_with("suite_contracts_") && name.ends_with(".rs") {
            out.push((name, fs::read_to_string(e.path()).unwrap()));
        }
    }
    out
}

#[test]
fn ctrx_ac05_lint_generator_and_partition_are_clean() {
    for args in [vec!["tests"], vec!["--fix", "tests"]] {
        let o = run("contract-lint.sh", &args);
        let t = text(&o);
        assert_eq!(o.status.code(), Some(0), "{args:?}: {t}");
        assert!(!t.contains("add  "), "{args:?}: {t}");
    }
    let o = run("gen-test-suites.sh", &["--check"]);
    assert_eq!(o.status.code(), Some(0), "{}", text(&o));
    let o = run("ci-test-partition.sh", &["check"]);
    assert_eq!(o.status.code(), Some(0), "{}", text(&o));
}

fn head(stem: &str) -> String {
    let src = fs::read_to_string(root().join(format!("tests/{stem}.rs"))).unwrap();
    src.lines().take(10).filter(|l| l.starts_with("//! contract:")).collect::<Vec<_>>().join("\n")
}

#[test]
fn ctrx_ac05_the_three_gap_files_and_ctrm_ac05_are_marked_and_in_one_contract_suite() {
    let suites = contract_suites();
    for stem in [
        "verbfwd_ac08_forward_stack_budget",
        "dryv_ac02_write_verb_registry_drift",
        "msmp_ac07_bridge_modules_list_sampling_everywhere",
        "ctrm_ac05_artifacts_printed_and_derived_not_listed",
    ] {
        assert!(!head(stem).is_empty(), "{stem} lacks a `//! contract:` line in its first 10 lines");
        let needle = format!("\"{stem}.rs\"");
        let homes: Vec<_> = suites.iter().filter(|(_, s)| s.contains(&needle)).map(|(n, _)| n.clone()).collect();
        assert_eq!(homes.len(), 1, "{stem} must be in exactly one contract suite: {homes:?}");
        let ok = (1..100).any(|n| homes[0] == format!("suite_contracts_core_{n:02}.rs") || homes[0] == format!("suite_contracts_sandbox_{n:02}.rs"));
        assert!(ok, "{stem} lives in {}", homes[0]);
    }
}

#[test]
fn ctrx_ac05_verbfwd_declares_one_registry_and_aliastruth_defines_the_aliases() {
    // Escapes keep the symbol names out of this file's own source, so it is not a walker of them.
    let verbfwd = head("verbfwd_ac08_forward_stack_budget");
    let decl = verbfwd.split('\u{2014}').next().unwrap();
    assert!(decl.contains("registry handler::host_tool_descriptor\x73"), "{verbfwd}");
    assert!(!decl.contains("FORWARD_DENYLIS\x54") && !decl.contains("TOOL_ALIASE\x53"), "{verbfwd}");
    assert_eq!(decl.matches("::").count(), 1, "verbfwd_ac08 declares more than one symbol: {verbfwd}");
    let aliases = head("aliastruth_ac01_tools_list_has_every_name");
    assert!(aliases.starts_with("//! contract: registry tool_aliases::TOOL_ALIASE\x53"), "{aliases}");
}

#[test]
fn ctrx_ac05_the_cargo_running_tests_stay_unmarked_and_outside_the_family() {
    let suites = contract_suites();
    for stem in ["ctrs_ac02_six_files_carry_marker_and_keep_tests", "ctrs_ac06_contract_suites_finish_within_budget"] {
        assert!(head(stem).is_empty(), "{stem} must stay unmarked");
        let needle = format!("\"{stem}.rs\"");
        assert!(!suites.iter().any(|(_, s)| s.contains(&needle)), "{stem} is in a contract suite");
    }
}

#[test]
fn ctrx_ac05_registries_list_the_walked_symbols() {
    let o = run("contract-lint.sh", &["--registries", "tests"]);
    let t = text(&o);
    assert_eq!(o.status.code(), Some(0), "{t}");
    for r in [
        "registry handler::host_tool_descriptor\x73",
        "registry tool_aliases::TOOL_ALIASE\x53",
        "registry write_verbs::WRITE_VERB\x53",
        "registry kinds::pytho\x6e::BRIDGE_MODULE\x53",
    ] {
        assert!(t.lines().any(|l| l == r), "{r} missing:\n{t}");
    }
}

fn suite_mods(files: Vec<String>) -> BTreeSet<String> {
    let mut mods = BTreeSet::new();
    for f in files {
        for l in f.lines() {
            if let Some(m) = l.strip_prefix("#[path = \"") {
                mods.insert(m.trim_end_matches("\"]").to_string());
            }
        }
    }
    mods
}

#[test]
fn ctrx_ac05_no_test_file_of_the_cited_tree_is_dropped() {
    let git = |args: &[&str]| Command::new("git").args(args).current_dir(root()).output().unwrap();
    if !git(&["cat-file", "-e", &format!("{CITED}^{{commit}}")]).status.success() {
        eprintln!("ctrx_ac05: {CITED} not in this clone's history; skipping");
        return;
    }
    let listing = git(&["ls-tree", "--name-only", CITED, "tests/"]);
    let mut before = Vec::new();
    for n in String::from_utf8_lossy(&listing.stdout).lines() {
        if n.starts_with("tests/suite_") && n.ends_with(".rs") {
            before.push(String::from_utf8_lossy(&git(&["show", &format!("{CITED}:{n}")]).stdout).into_owned());
        }
    }
    let after: Vec<String> = fs::read_dir(root().join("tests"))
        .unwrap()
        .flatten()
        .filter(|e| {
            let n = e.file_name().to_string_lossy().into_owned();
            n.starts_with("suite_") && n.ends_with(".rs")
        })
        .map(|e| fs::read_to_string(e.path()).unwrap())
        .collect();
    let (before, after) = (suite_mods(before), suite_mods(after));
    assert!(!before.is_empty());
    let lost: Vec<_> = before.difference(&after).collect();
    assert!(lost.is_empty(), "test files dropped from every suite: {lost:?}");
}
