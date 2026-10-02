//! PRD-mcphost-test-suite-flake-lints
//! AC6 (P0) — Given `MCPHOST_PERF_SKIP=1`, When a `perf_budget!` test runs,
//! Then it passes and prints `perf skipped (load)`; given it unset and a
//! body that takes 60 ms median against 50, Then it fails with the median
//! in the message.
//!
//! The two `flakelint_ac06_subject_*` fns below are deliberately inert
//! under a normal `cargo test` run (each returns immediately unless its own
//! marker env var is set) -- the driver tests re-run exactly one of them at
//! a time in a fresh child process with the right env, `--nocapture` (so
//! `perf_budget!`'s `eprintln!` isn't swallowed by libtest's own capture),
//! and inspect the child's exit status and combined output. Same
//! self-exec-one-test convention
//! `tests/sessbind_ac08_binding_map_is_bounded_under_a_session_flood.rs`
//! uses to isolate a process-level measurement from the rest of the suite.

use std::process::Command;
use std::time::Duration;

const SKIP_SUBJECT: &str =
    "flakelint_ac06_perf_budget_skip_and_median::flakelint_ac06_subject_skip";
const SLOW_SUBJECT: &str =
    "flakelint_ac06_perf_budget_skip_and_median::flakelint_ac06_subject_slow";

/// Only does real work when driven with `MCPHOST_PERF_SKIP=1` -- the exact
/// var `perf_budget!` itself checks, so this subject is a safe no-op under
/// an ordinary suite run regardless of whether the ambient gate happens to
/// have that var set.
#[test]
fn flakelint_ac06_subject_skip() {
    if std::env::var("MCPHOST_PERF_SKIP").ok().as_deref() != Some("1") {
        return;
    }
    crate::perf_budget!(1, {
        std::thread::sleep(Duration::from_millis(60));
    });
}

/// Only does real work when driven with its own marker set -- an ordinary
/// suite run (marker unset) must never pay this 60 ms sleep or risk a
/// surprise failure from some ambient `MCPHOST_PERF_SKIP` state.
#[test]
fn flakelint_ac06_subject_slow() {
    if std::env::var("FLAKELINT_AC06_RUN_SLOW_SUBJECT").is_err() {
        return;
    }
    crate::perf_budget!(50, {
        std::thread::sleep(Duration::from_millis(60));
    });
}

fn run_subject(name: &str, envs: &[(&str, &str)], remove_envs: &[&str]) -> std::process::Output {
    let exe = std::env::current_exe().expect("this test binary's own path");
    let mut cmd = Command::new(exe);
    cmd.args(["--exact", name, "--nocapture"]);
    for (k, v) in envs {
        cmd.env(k, v);
    }
    for k in remove_envs {
        cmd.env_remove(k);
    }
    cmd.output().expect("re-run one subject test in a child process")
}

#[test]
fn skip_mode_passes_and_prints_perf_skipped_load() {
    let out = run_subject(SKIP_SUBJECT, &[("MCPHOST_PERF_SKIP", "1")], &[]);
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
    assert!(
        out.status.success(),
        "perf_budget! under MCPHOST_PERF_SKIP=1 must pass, but the child exited non-zero:\n{combined}"
    );
    assert!(
        combined.contains("perf skipped (load)"),
        "perf_budget! under MCPHOST_PERF_SKIP=1 must print the skip line; child output:\n{combined}"
    );
}

#[test]
fn unset_and_slow_fails_with_the_median_in_the_message() {
    let out = run_subject(
        SLOW_SUBJECT,
        &[("FLAKELINT_AC06_RUN_SLOW_SUBJECT", "1")],
        &["MCPHOST_PERF_SKIP"],
    );
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
    assert!(
        !out.status.success(),
        "a 60ms-median body against a 50ms budget must fail, but the child exited \
         successfully:\n{combined}"
    );
    assert!(
        combined.contains("median"),
        "the failure must report the median in its message; child output:\n{combined}"
    );
}
