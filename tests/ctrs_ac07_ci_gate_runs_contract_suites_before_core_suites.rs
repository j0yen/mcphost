//! PRD-mcphost-contract-suite-generated AC7 -- `.github/workflows/ci.yml`'s
//! `core-contract` job (named `contract suites`, aggregated by `gate`, its own
//! job since #495 hit core-static's 20-minute cap) has a step named
//! `contract suites` that runs `scripts/pin-tests.sh`; `core-static` no longer
//! runs it, and the core shards do not run it a second time as a step.

use std::fs;
use std::path::PathBuf;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// The block of job `id`: from `  <id>:` to the next top-level job key.
fn job_block<'a>(yml: &'a str, id: &str) -> &'a str {
    let header = format!("\n  {id}:\n");
    let start = yml.find(&header).unwrap_or_else(|| panic!("ci.yml has a {id} job")) + 1;
    let rest = &yml[start + header.len() - 1..];
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
    &rest[..end]
}

#[test]
fn ctrs_ac07_contract_suites_step_runs_pin_tests_in_its_own_job() {
    let yml = fs::read_to_string(root().join(".github/workflows/ci.yml")).unwrap();
    let job = job_block(&yml, "core-contract");
    assert!(job.lines().any(|l| l.trim() == "name: contract suites"), "the job is named `contract suites`:\n{job}");

    let steps: Vec<&str> = job.lines().filter(|l| l.trim_start().starts_with("- name:")).collect();
    assert!(
        steps.iter().any(|l| l.trim_start() == "- name: contract suites"),
        "core-contract job has a `contract suites` step: {steps:?}"
    );

    let at = job.find("- name: contract suites").unwrap();
    let step_body: String = job[at..].lines().skip(1).take_while(|l| !l.trim_start().starts_with("- name:")).collect::<Vec<_>>().join("\n");
    assert!(step_body.contains("scripts/pin-tests.sh"), "step must run scripts/pin-tests.sh:\n{step_body}");
}

#[test]
fn ctrs_ac07_core_static_and_the_core_shards_do_not_run_pin_tests_as_a_step() {
    let yml = fs::read_to_string(root().join(".github/workflows/ci.yml")).unwrap();
    let stat = job_block(&yml, "core-static");
    assert!(!stat.contains("- name: contract suites"), "core-static must not carry a `contract suites` step:\n{stat}");
    assert!(!stat.contains("pin-tests.sh"), "core-static must not run pin-tests.sh");
    let core = job_block(&yml, "core");
    assert!(
        !core.contains("- name: contract suites") && !core.lines().any(|l| l.trim() == "run: ./scripts/pin-tests.sh"),
        "the core shards do not run pin-tests.sh as a step (they only list its binaries with --list)"
    );
}
