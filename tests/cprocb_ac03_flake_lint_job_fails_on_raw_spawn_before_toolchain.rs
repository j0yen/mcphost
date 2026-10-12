//! PRD-mcphost-child-process-test-harness-b AC3 — a PR that adds a raw `serve`
//! spawn in a test fails the `flake-lint` CI job naming file:line, and no
//! Rust toolchain step precedes the lint step in that job.

use crate::child_proc_lint;

use child_proc_lint::{fixture, repo_root, run, scratch, write};
use std::fs;

fn flake_lint_job() -> String {
    let ci = fs::read_to_string(repo_root().join(".github/workflows/ci.yml")).expect("read ci.yml");
    let start = ci.find("\n  flake-lint:\n").expect("flake-lint job present");
    let rest = &ci[start + 1..];
    let end = rest
        .match_indices("\n\n  ")
        .find(|(i, _)| !rest[i + 4..].starts_with(' '))
        .map_or(rest.len(), |(i, _)| i);
    rest[..end].to_string()
}

#[test]
fn job_runs_the_lint_and_no_toolchain_step_exists_before_it() {
    let job = flake_lint_job();
    let lint_at = job
        .find("run: python3 scripts/child-proc-lint.py tests")
        .expect("flake-lint job runs child-proc-lint.py");
    let before = &job[..lint_at];
    for forbidden in ["rust-toolchain", "rustup", "cargo", "rust-cache", "needs:"] {
        assert!(!before.contains(forbidden), "{forbidden} precedes the lint step:\n{before}");
    }
}

#[test]
fn a_pr_adding_a_raw_serve_spawn_fails_naming_file_and_line() {
    let dir = scratch("cprocb-ac03");
    write(&dir, "pr_adds_raw_spawn.rs", &fixture("hand_rolled_serve.rs.tpl"));
    let (code, out) = run(&dir);
    assert_eq!(code, 1, "{out}");
    let named = out
        .lines()
        .any(|l| l.starts_with("pr_adds_raw_spawn.rs:") || l.contains("/pr_adds_raw_spawn.rs:"));
    assert!(named, "output must name file:line:\n{out}");
}
