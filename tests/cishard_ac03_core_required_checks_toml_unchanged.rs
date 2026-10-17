//! PRD-mcphost-ci-core-suites-sharded AC3 -- `scripts/required-checks.sh
//! --print` equals the committed .github/required-checks.toml byte for byte
//! (13 contexts), because the static and shard jobs carry the
//! `# required-checks: exclude` marker; no admin `--apply` is needed.

use crate::ci_sandbox_support;
use ci_sandbox_support as support;
use std::path::PathBuf;
use std::process::Command;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

#[test]
fn print_equals_the_committed_toml_byte_for_byte() {
    let out = Command::new(root().join("scripts/required-checks.sh"))
        .arg("--print")
        .current_dir(root())
        .output()
        .expect("run scripts/required-checks.sh --print");
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let committed = std::fs::read(root().join(".github/required-checks.toml")).expect("committed toml");
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&committed),
        "the workflow's rendered contexts drifted from .github/required-checks.toml"
    );
}

#[test]
fn the_toml_still_lists_the_aggregator_and_thirteen_contexts_and_no_shard() {
    let toml = std::fs::read_to_string(root().join(".github/required-checks.toml")).unwrap();
    let contexts: Vec<&str> = toml.lines().filter(|l| l.starts_with("  \"")).collect();
    assert_eq!(contexts.len(), 13, "{contexts:?}");
    assert!(toml.contains("\"static analysis + core suites\""));
    assert!(!toml.contains("core suites (shard"), "a core shard must never be a required context");
    assert!(!toml.contains("\"static analysis\""), "the static job is covered by the aggregator");
}

#[test]
fn the_static_and_shard_jobs_carry_the_exclude_marker() {
    let jobs = support::jobs(&support::workflow());
    for name in ["core-static", "core"] {
        let (_, body) = jobs
            .iter()
            .find(|(n, _)| n == name)
            .unwrap_or_else(|| panic!("ci.yml has no `{name}` job"));
        assert!(
            body.lines().any(|l| l.trim_start().starts_with("# required-checks: exclude")),
            "job {name} must carry `# required-checks: exclude`"
        );
    }
    let (_, gate) = jobs.iter().find(|(n, _)| n == "gate").expect("gate job");
    assert!(
        !gate.contains("required-checks: exclude"),
        "the aggregator is THE required context and must not be excluded"
    );
}
