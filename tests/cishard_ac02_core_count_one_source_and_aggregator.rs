//! contract: the core shard matrix is generated and the aggregator keeps the required name
//! PRD-mcphost-ci-core-suites-sharded AC2 -- ci.yml's core matrix comes from
//! the plan job's `ci-test-partition.sh shards core` output, no literal core
//! count appears in ci.yml or gen-test-suites.sh, and the job named exactly
//! "static analysis + core suites" needs the static job and the core matrix
//! with `if: always()` and fails unless both results are success.

use crate::ci_sandbox_support;
use ci_sandbox_support as support;

fn job(name: &str) -> String {
    support::jobs(&support::workflow())
        .into_iter()
        .find(|(n, _)| n == name)
        .unwrap_or_else(|| panic!("ci.yml has no `{name}` job"))
        .1
}

/// Literal core-count shapes that must never appear in the consumers.
fn literal_core_counts(text: &str) -> Vec<String> {
    let mut hits = Vec::new();
    for (i, line) in text.lines().enumerate() {
        let digit_after = |s: &str| {
            line.find(s)
                .map(|p| line[p + s.len()..].trim_start().starts_with(|c: char| c.is_ascii_digit()))
                .unwrap_or(false)
        };
        let bad = digit_after("CORE_SHARDS=")
            || digit_after("CORE_SHARDS =")
            || digit_after("core_shards=")
            || (line.contains("core_matrix=") && line.contains("[1"));
        if bad {
            hits.push(format!("{}: {}", i + 1, line.trim()));
        }
    }
    hits
}

#[test]
fn the_count_is_defined_once_and_printed_by_shards_core() {
    let script = support::read_repo_file("scripts/ci-test-partition.sh");
    let defs: Vec<&str> = script
        .lines()
        .filter(|l| l.trim_start().starts_with("CORE_SHARDS="))
        .collect();
    assert_eq!(defs.len(), 1, "exactly one CORE_SHARDS definition: {defs:?}");
    let printed = support::partition(&["shards", "core"]);
    assert!(printed.trim().parse::<u32>().is_ok(), "`shards core` prints a count: {printed:?}");
    // Bare `shards` keeps its sandbox meaning (both original callers call it bare).
    let bare = support::partition(&["shards"]);
    let sandbox = support::partition(&["shards", "sandbox"]);
    assert_eq!(bare, sandbox);
}

#[test]
fn the_plan_job_emits_the_core_matrix_from_the_command() {
    let plan = job("plan");
    assert!(plan.contains("./scripts/ci-test-partition.sh shards core"), "plan must call `shards core`:\n{plan}");
    assert!(plan.contains("core_matrix:"), "plan must output core_matrix");
    assert!(plan.contains("core_shards:"), "plan must output core_shards");
    let core = job("core");
    assert!(
        core.contains("matrix: ${{ fromJson(needs.plan.outputs.core_matrix) }}"),
        "the core matrix must come from the plan job:\n{core}"
    );
    assert!(
        core.contains("core-shard \"$shard\" \"$total\""),
        "each shard runs core-shard <n> <total>:\n{core}"
    );
    assert!(
        core.contains("/${{ needs.plan.outputs.core_shards }})"),
        "the job name total must come from the plan job"
    );
}

#[test]
fn no_literal_core_count_in_ci_yml_or_gen_test_suites() {
    for rel in [".github/workflows/ci.yml", "scripts/gen-test-suites.sh"] {
        let hits = literal_core_counts(&support::read_repo_file(rel));
        assert!(hits.is_empty(), "{rel} carries a literal core shard count:\n{}", hits.join("\n"));
    }
    let gen_script = support::read_repo_file("scripts/gen-test-suites.sh");
    assert!(
        gen_script.contains(r#"[CLASSIFY_SCRIPT, "shards", "core"]"#),
        "gen-test-suites.sh must read `ci-test-partition.sh shards core`"
    );
}

#[test]
fn the_drift_scan_catches_a_reintroduced_core_literal() {
    assert!(!literal_core_counts("# CORE_SHARDS=2 matrix").is_empty());
    assert!(!literal_core_counts("echo core_shards=2 >> $GITHUB_OUTPUT").is_empty());
    assert!(literal_core_counts("core_shards=$(./scripts/ci-test-partition.sh shards core)").is_empty());
}

#[test]
fn the_aggregator_keeps_id_and_name_and_needs_both_jobs_always() {
    let gate = job("gate");
    assert!(
        gate.lines().any(|l| l.trim() == "name: static analysis + core suites"),
        "the aggregator keeps the exact required name:\n{gate}"
    );
    let needs = gate
        .lines()
        .find(|l| l.trim_start().starts_with("needs:"))
        .expect("the aggregator declares needs");
    assert!(needs.contains("core-static") && needs.contains("core]"), "needs both jobs: {needs}");
    assert!(
        gate.lines().any(|l| l.trim() == "if: always()"),
        "the aggregator must run even when a job it needs failed"
    );
    assert!(gate.contains("needs.core-static.result"), "must read the static job's result");
    assert!(gate.contains("needs.core.result"), "must read the core matrix's result");
    assert!(gate.contains(r#"!= "success""#), "must fail unless both results are success");
    assert!(gate.contains("exit 1"));
    assert!(!gate.contains("cargo "), "the aggregator runs no tests of its own");
    // The static job and the shards each own what the old job did.
    assert!(job("core-static").contains("./scripts/ci-test-partition.sh check"));
    assert!(job("core-static").contains("./scripts/pin-tests.sh"));
    assert!(job("core-static").contains("cargo test --locked --doc"));
    assert!(job("core-static").contains("cargo deny --locked check"));
    assert!(job("core-static").contains("./scripts/audit.sh"));
}
