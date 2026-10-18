//! contract: contract-lint.sh --artifacts prints only derived repo paths
//! PRD-mcphost-contract-suite-markers AC5 -- `scripts/contract-lint.sh
//! --artifacts` prints one repo-relative path per line (including the intent
//! card, the changelog and the suite weights), every printed path is read by at
//! least one marked test, and the script's source carries no literal signature
//! array.

use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn artifacts() -> Vec<String> {
    let out = Command::new(root().join("scripts/contract-lint.sh"))
        .arg("--artifacts")
        .current_dir(root())
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    String::from_utf8_lossy(&out.stdout).lines().map(str::to_string).collect()
}

/// Files with a `//! contract:` line (not a follower `reads`/`walks` line).
fn defining_marked_sources() -> Vec<String> {
    let mut out = Vec::new();
    for entry in fs::read_dir(root().join("tests")).unwrap() {
        let p = entry.unwrap().path();
        if p.extension().map_or(true, |e| e != "rs") {
            continue;
        }
        let src = fs::read_to_string(&p).unwrap();
        let marker = src.lines().take(10).find_map(|l| l.strip_prefix("//! contract:"));
        if let Some(m) = marker {
            let m = m.trim();
            if !m.starts_with("reads ") && !m.starts_with("walks ") {
                out.push(src);
            }
        }
    }
    out
}

#[test]
fn ctrm_ac05_prints_the_expected_paths_one_per_line() {
    let got = artifacts();
    for want in [format!("agent/{}", "intent-card.json"), format!("{}.md", "CHANGE".to_string() + "LOG"), format!("scripts/ci-suite-{}", "weights.json")] {
        assert!(got.contains(&want), "{want} missing from {got:?}");
    }
    for line in &got {
        assert!(!line.starts_with('/') && !line.contains(' ') && root().join(line).is_file(), "{line}");
    }
}

#[test]
fn ctrm_ac05_every_printed_path_is_read_by_a_marked_test() {
    let marked = defining_marked_sources();
    for path in artifacts() {
        let leaf = path.rsplit('/').next().unwrap();
        assert!(marked.iter().any(|src| src.contains(leaf)), "no marked test reads {path}");
    }
}

#[test]
fn ctrm_ac05_script_has_no_literal_signature_array() {
    let src = fs::read_to_string(root().join("scripts/contract-lint.sh")).unwrap();
    for needle in ["RULES=(", "SIGNATURES", "CHANGELOG", "migrations", "weights"] {

        assert!(!src.contains(needle), "script source contains {needle}");
    }
}
