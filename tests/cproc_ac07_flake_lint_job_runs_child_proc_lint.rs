//! PRD-mcphost-child-process-test-harness AC7 — the `flake-lint` CI job runs
//! `scripts/child-proc-lint.py tests` with no Rust toolchain step anywhere in
//! the job, and the script it runs names `file:line` for a raw `serve` spawn
//! (so a PR adding one fails the job with that line in its log).

use crate::child_proc_lint;

use child_proc_lint::{fixture, repo_root, run, scratch, write};
use std::fs;

/// The text of the `flake-lint:` job: from its key to the next top-level job.
fn flake_lint_job() -> String {
    let ci = fs::read_to_string(repo_root().join(".github/workflows/ci.yml")).expect("read ci.yml");
    let start = ci.find("\n  flake-lint:\n").expect("flake-lint job present");
    let rest = &ci[start + 1..];
    // The job ends at the first blank line followed by a two-space-indented
    // key or comment (the next job's header block).
    let end = rest
        .match_indices("\n\n  ")
        .find(|(i, _)| !rest[i + 4..].starts_with(' '))
        .map_or(rest.len(), |(i, _)| i);
    rest[..end].to_string()
}

#[test]
fn flake_lint_job_runs_the_script_and_installs_no_toolchain() {
    let job = flake_lint_job();
    assert!(
        job.contains("scripts/child-proc-lint.py tests"),
        "flake-lint job must run the script:\n{job}"
    );
    for forbidden in ["rust-toolchain", "rustup", "cargo", "rust-cache"] {
        assert!(
            !job.contains(forbidden),
            "flake-lint job must not touch a Rust toolchain ({forbidden}):\n{job}"
        );
    }
    assert!(!job.contains("needs:"), "flake-lint must not wait on other jobs:\n{job}");
}

#[test]
fn a_raw_serve_spawn_is_reported_with_file_and_line() {
    let dir = scratch("ac07");
    write(&dir, "pr_adds_raw_spawn.rs", &fixture("hand_rolled_serve.rs.tpl"));
    let (code, out) = run(&dir);
    assert_eq!(code, 1, "{out}");
    assert!(out.contains("pr_adds_raw_spawn.rs:"), "output must name file:line:\n{out}");
}
