//! PRD-mcphost-contract-suite-generated AC7 -- `.github/workflows/ci.yml`'s
//! `core-static` job (the static half of the required "static analysis + core
//! suites" check since PRD-mcphost-ci-core-suites-sharded) has a step named
//! `contract suites` that runs `scripts/pin-tests.sh` before its `cargo test
//! --doc` step, and the core shards do not run it a second time as a step.

use std::fs;
use std::path::PathBuf;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

#[test]
fn ctrs_ac07_contract_suites_step_precedes_core_suites_in_the_gate_job() {
    let yml = fs::read_to_string(root().join(".github/workflows/ci.yml")).unwrap();
    // The `core-static` job block: from `  core-static:` to the next top-level job key.
    let start = yml.find("\n  core-static:\n").expect("ci.yml has a core-static job") + 1;
    let rest = &yml[start + "  core-static:\n".len()..];
    let end = rest
        .lines()
        .scan(0usize, |off, l| {
            let here = *off;
            *off += l.len() + 1;
            Some((here, l))
        })
        .find(|(_, l)| l.starts_with("  ") && !l.starts_with("   ") && l.trim_end().ends_with(':') && !l.trim().starts_with('#'))
        .map(|(o, _)| o)
        .unwrap_or(rest.len());
    let gate = &rest[..end];

    let steps: Vec<&str> = gate.lines().filter(|l| l.trim_start().starts_with("- name:")).collect();
    let pos = |name: &str| steps.iter().position(|l| l.trim_start() == format!("- name: {name}"));
    let contract = pos("contract suites").expect("core-static job has a `contract suites` step");
    let doc = pos("cargo test --doc").expect("core-static job has a `cargo test --doc` step");
    assert!(contract < doc, "`contract suites` must precede `cargo test --doc`: {steps:?}");

    let at = gate.find("- name: contract suites").unwrap();
    let step_body: String = gate[at..].lines().skip(1).take_while(|l| !l.trim_start().starts_with("- name:")).collect::<Vec<_>>().join("\n");
    assert!(step_body.contains("scripts/pin-tests.sh"), "step must run scripts/pin-tests.sh:\n{step_body}");
}
