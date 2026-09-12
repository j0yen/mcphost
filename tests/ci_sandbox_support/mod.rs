//! PRD-mcphost-ci-sandbox-coverage -- shared support for the `ci_sandbox_ac*`
//! acceptance tests.
//!
//! Lives in a subdirectory so cargo's `tests/*.rs` auto-discovery does not
//! turn it into a test target of its own (the same reason `tests/common/`
//! is a directory), and so `scripts/ci-test-partition.sh`, which partitions
//! exactly `tests/*.rs`, never has to special-case it.
//!
//! ## Why the guard tests re-invoke this binary as a child
//!
//! ACs 2-5 are four cells of one truth table over (capability present?,
//! `$CI` set?). Both inputs are process-global environment reads:
//! `supports_user_namespaces()` execs `unshare` off `$PATH`, and the guard
//! reads `$CI`. Under edition 2024 `std::env::set_var` is `unsafe` and racy
//! against every other thread in a libtest binary, so an in-process test
//! cannot vary either input honestly -- which is why ACs 3 and 4 shipped
//! through v0.13.3 with no automated test at all, only a manual code read.
//!
//! Re-invoking the test binary as a child process with a controlled
//! environment varies both inputs for real, exercising the actual public
//! `require_user_namespaces_or_ci_skip()` rather than a refactored-out
//! decision function. Capability is removed by handing the child a `$PATH`
//! with no `unshare` on it, which makes the probe's `Command::status()` fail
//! with ENOENT and the probe return `false` through its own `unwrap_or(false)`
//! -- the same answer a kernel that denies `CLONE_NEWUSER` produces, reached
//! by the same code path.
#![allow(dead_code)]

use std::path::PathBuf;
use std::process::{Command, Output};

/// Set in the child; its presence is what makes a `ci_sandbox_ac0*` test take
/// the child role instead of the assertion role.
pub const CHILD_ROLE_ENV: &str = "MCPHOST_CI_SANDBOX_GUARD_CHILD";

/// The child prints this, followed by the guard's return value, only if the
/// guard RETURNED. A panicking guard never reaches it -- which is exactly
/// what AC4 asserts.
pub const GUARD_RESULT_PREFIX: &str = "guard-returned: ";

/// Strip the leading crate-root segment `module_path!()` always carries
/// (e.g. `"suite_sandbox_01::ci_sandbox_ac02_capable_env_never_skips"`) down
/// to the module path libtest itself uses for `--exact` filtering (e.g.
/// `"ci_sandbox_ac02_capable_env_never_skips"`) -- PRD-mcphost-test-suite-
/// consolidation: since every `tests/*.rs` file is now `#[path]`-included one
/// level under a generated `tests/suite_*.rs` crate root, `module_path!()`
/// inside a test function carries that suite's name as its first segment,
/// which libtest's own test names never include (nextest lists them as
/// `<binary> <module-path-without-the-binary>`). Used by the `ci_sandbox_ac*`
/// guard tests to build the exact name they re-invoke themselves with.
pub fn strip_crate_root(module_path: &str) -> &str {
    module_path.split_once("::").map_or(module_path, |(_, rest)| rest)
}

pub fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

pub fn read_repo_file(rel: &str) -> String {
    let path = repo_root().join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

pub fn workflow() -> String {
    read_repo_file(".github/workflows/ci.yml")
}

/// Run `scripts/ci-test-partition.sh <args>` and return its stdout, asserting
/// it exited 0.
pub fn partition(args: &[&str]) -> String {
    let out = Command::new(repo_root().join("scripts/ci-test-partition.sh"))
        .args(args)
        .current_dir(repo_root())
        .output()
        .expect("run scripts/ci-test-partition.sh");
    assert!(
        out.status.success(),
        "ci-test-partition.sh {args:?} exited {:?}\nstdout: {}\nstderr: {}",
        out.status.code(),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// The child role: call the real guard exactly the way every sandbox test
/// calls it, report what happened, and never return to libtest.
pub fn child_report_guard() -> ! {
    let skipped = mcphost::sandbox::require_user_namespaces_or_ci_skip();
    if skipped {
        // Byte-for-byte the line a skipping sandbox test emits, so AC3 can
        // assert the workflow's grep would match a real skip.
        println!("{} (CI)", mcphost::sandbox::USERNS_SKIP_MARKER);
    }
    println!("{GUARD_RESULT_PREFIX}{skipped}");
    std::process::exit(0);
}

/// Re-invoke this test binary running only `test_name`, with the capability
/// probe and `$CI` forced to the requested combination.
pub fn run_guard_child(test_name: &str, capable: bool, ci: bool) -> Output {
    let exe = std::env::current_exe().expect("current_exe");
    let mut cmd = Command::new(&exe);
    cmd.args([test_name, "--exact", "--nocapture", "--test-threads=1"])
        .env(CHILD_ROLE_ENV, "1");

    if capable {
        // Inherit `$PATH` so the probe finds `unshare` for real. On a box
        // that genuinely lacks user namespaces this child panics and the
        // calling test fails loudly -- the 2026-09-03 dev-box contract,
        // applied to this suite too rather than special-cased out of it.
    } else {
        let empty = std::env::temp_dir().join(format!(
            "mcphost-ci-sandbox-nopath-{}-{}",
            std::process::id(),
            test_name,
        ));
        std::fs::create_dir_all(&empty).expect("scratch PATH dir");
        cmd.env("PATH", &empty);
    }

    if ci {
        cmd.env("CI", "true");
    } else {
        cmd.env_remove("CI");
    }

    cmd.output()
        .unwrap_or_else(|e| panic!("re-invoke {} as guard child: {e}", exe.display()))
}

pub fn stdout_of(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

pub fn stderr_of(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

/// Split the workflow into `(job-name, job-body)` pairs. Hand-rolled rather
/// than pulled in as a YAML dependency: this crate has no YAML crate, adding
/// one to `[dev-dependencies]` for four shape assertions would widen the
/// supply-chain audit's surface, and the file being parsed is one we own and
/// keep two-space-indented.
pub fn jobs(workflow: &str) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    let mut in_jobs = false;
    for line in workflow.lines() {
        if line == "jobs:" {
            in_jobs = true;
            continue;
        }
        if !in_jobs {
            continue;
        }
        // A top-level key ends the `jobs:` mapping.
        if !line.starts_with(' ') && !line.trim().is_empty() && !line.starts_with('#') {
            break;
        }
        let is_job_header = line.starts_with("  ")
            && !line.starts_with("   ")
            && line.trim_end().ends_with(':')
            && !line.trim_start().starts_with('#');
        if is_job_header {
            let name = line.trim().trim_end_matches(':').to_string();
            out.push((name, String::new()));
        } else if let Some(last) = out.last_mut() {
            last.1.push_str(line);
            last.1.push('\n');
        }
    }
    out
}

/// `true` if this job body declares a `needs:` dependency on another job.
pub fn declares_needs(job_body: &str) -> bool {
    job_body
        .lines()
        .any(|l| l.trim_start().starts_with("needs:") && !l.trim_start().starts_with('#'))
}
