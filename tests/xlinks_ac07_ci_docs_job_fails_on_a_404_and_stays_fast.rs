//! AC7 (PRD-mcphost-docs-external-links-resolve) -- Given
//! `.github/workflows/ci.yml`, When a PR introduces a 404 link, Then the
//! docs job fails with the URL in the log; a green PR's docs job finishes
//! in <= 60 s.
//!
//! What a test can prove here: the `docs` job exists, runs the checker as
//! its step, needs no Rust toolchain / sandbox partition / `needs:` (so it
//! cannot be slowed by, or wait for, the build jobs), and that exact command
//! exits non-zero with the URL on stdout when a doc carries a 404. The
//! wall-clock half is a property of a real runner and is reported as
//! deferred; the job carries a 5 minute hard timeout and the checker's
//! per-URL timeout is 10 s, 8 in parallel.

use crate::xlinks;

/// Body of job `name` in ci.yml (two-space-indented keys under `jobs:`).
fn job(name: &str) -> String {
    let wf = std::fs::read_to_string(xlinks::repo_root().join(".github/workflows/ci.yml")).unwrap();
    let after_jobs = &wf[wf.find("\njobs:\n").expect("ci.yml declares jobs:")..];
    let header = format!("\n  {name}:\n");
    let start = after_jobs.find(&header).unwrap_or_else(|| panic!("ci.yml must declare job `{name}`")) + header.len();
    let rest = &after_jobs[start..];
    let end = rest
        .lines()
        .scan(0usize, |off, l| {
            let at = *off;
            *off += l.len() + 1;
            Some((at, l))
        })
        .find(|(_, l)| l.starts_with("  ") && !l.starts_with("   ") && l.trim_end().ends_with(':') && !l.trim_start().starts_with('#'))
        .map(|(at, _)| at)
        .unwrap_or(rest.len());
    // Comments (the next job's lead-in sits at the end of this slice) are not config.
    rest[..end].lines().filter(|l| !l.trim_start().starts_with('#')).collect::<Vec<_>>().join("\n")
}

#[test]
fn docs_job_runs_the_checker_with_no_toolchain_and_no_needs() {
    let docs = job("docs");
    assert!(docs.contains("./scripts/docs-link-check.sh"), "{docs}");
    assert!(!docs.contains("needs:"), "docs job must run concurrently: {docs}");
    for slow in ["cargo", "rust-toolchain", "ci-test-partition", "bubblewrap", "unshare"] {
        assert!(!docs.contains(slow), "docs job must stay fast and sandbox-free, found `{slow}`: {docs}");
    }
    let timeout: u32 = docs
        .lines()
        .find_map(|l| l.trim().strip_prefix("timeout-minutes:").map(|v| v.trim().parse().unwrap()))
        .expect("docs job needs a timeout");
    assert!(timeout <= 5, "timeout-minutes {timeout}");
}

#[test]
fn clients_drift_check_runs_in_ci_as_its_own_job() {
    let drift = job("docs-clients");
    assert!(drift.contains("llms-txt --check"), "{drift}");
    assert!(!drift.contains("needs:"), "{drift}");
}

#[test]
fn the_docs_step_command_fails_a_404_and_prints_the_url() {
    let docs = job("docs");
    let step = docs
        .lines()
        .find_map(|l| l.trim().strip_prefix("run: ").map(str::to_string))
        .expect("docs job has a run step");
    assert_eq!(step, "./scripts/docs-link-check.sh", "the docs step is the bare checker");

    let port = xlinks::serve();
    let dir = xlinks::scratch("ac07");
    let file = dir.join("pr.md");
    let gone = format!("http://127.0.0.1:{port}/gone");
    std::fs::write(&file, format!("a new link: {gone}\n")).unwrap();
    let out = xlinks::run_checker(&[&file]);
    assert_eq!(out.status.code(), Some(1));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains(&format!("FAIL 404 {gone}")), "the URL must be in the log: {stdout}");
}

#[test]
fn a_green_pr_checks_a_large_url_set_within_the_60_second_budget() {
    let port = xlinks::serve();
    let dir = xlinks::scratch("ac07-green");
    let file = dir.join("pr.md");
    let body: String = (0..120).map(|i| format!("- http://127.0.0.1:{port}/ok?n={i}\n")).collect();
    std::fs::write(&file, body).unwrap();
    let start = std::time::Instant::now();
    let out = xlinks::run_checker(&[&file]);
    let took = start.elapsed();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(0), "a green set must pass: {stdout}");
    assert!(!stdout.contains("FAIL"), "{stdout}");
    assert!(took < std::time::Duration::from_secs(60), "120 URLs took {took:?}, budget is 60 s");
}
