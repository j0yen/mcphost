//! PRD-mcphost-ci-sandbox-shards-balanced AC3 -- a sandbox shard job reports
//! `build wall` and `run wall` in the step summary, warns at run wall >= 240 s,
//! fails only above 300 s, and on warn or fail prints one
//! `suite=<name> measured=<s>s recorded=<s>s` line per suite.

use std::path::PathBuf;
use std::process::{Command, Output};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

const LOG: &str = "\
     Running unittests src/lib.rs (target/debug/deps/mcphost-aaaa)
test result: ok. 314 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 5.70s
     Running tests/suite_sandbox_02.rs (target/debug/deps/suite_sandbox_02-bbbb)
test result: ok. 289 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 152.20s
     Running tests/suite_env_sandbox_01.rs (target/debug/deps/suite_env_sandbox_01-cccc)
test result: ok. 9 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 7.10s
";

struct Run {
    out: Output,
    summary: String,
}

fn report(tag: &str, build_s: u32, run_s: u32, status: u32) -> Run {
    let dir = std::env::temp_dir().join(format!("cishard-ac03-{}-{tag}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let log = dir.join("cargo-test.log");
    let summary = dir.join("summary.md");
    std::fs::write(&log, LOG).unwrap();
    std::fs::write(&summary, "").unwrap();
    let out = Command::new(root().join("scripts/ci-shard-report.sh"))
        .args(["2", "3", &build_s.to_string(), &run_s.to_string()])
        .arg(&log)
        .arg(status.to_string())
        .env("GITHUB_STEP_SUMMARY", &summary)
        .current_dir(root())
        .output()
        .expect("run scripts/ci-shard-report.sh");
    let summary_text = std::fs::read_to_string(&summary).unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    Run { out, summary: summary_text }
}

fn stdout(r: &Run) -> String {
    String::from_utf8_lossy(&r.out.stdout).into_owned()
}

#[test]
fn the_step_summary_carries_build_wall_and_run_wall() {
    let r = report("summary", 95, 120, 0);
    assert!(r.out.status.success(), "{}", stdout(&r));
    assert!(r.summary.contains("sandbox shard 2/3 build wall: 95s"), "{}", r.summary);
    assert!(r.summary.contains("sandbox shard 2/3 run wall: 120s"), "{}", r.summary);
}

#[test]
fn under_240_s_is_silent_no_warning_no_suite_lines() {
    let r = report("quiet", 400, 239, 0);
    assert!(r.out.status.success());
    let out = stdout(&r);
    assert!(!out.contains("::warning::") && !out.contains("::error::"), "{out}");
    assert!(!out.contains("suite="), "{out}");
}

#[test]
fn build_wall_is_never_budgeted() {
    let r = report("slowbuild", 900, 100, 0);
    assert!(r.out.status.success(), "{}", stdout(&r));
}

#[test]
fn at_240_s_warns_and_passes_with_one_line_per_suite() {
    let r = report("warn", 90, 240, 0);
    assert!(r.out.status.success(), "a warn must not fail the step: {}", stdout(&r));
    let out = stdout(&r);
    assert!(out.contains("::warning::"), "{out}");
    assert!(!out.contains("::error::"), "{out}");
    for line in [
        "suite=lib measured=5.70s recorded=6s",
        "suite=suite_sandbox_02 measured=152.20s recorded=152s",
        "suite=suite_env_sandbox_01 measured=7.10s recorded=7s",
    ] {
        assert!(out.contains(line), "missing `{line}` in:\n{out}");
        assert!(r.summary.contains(line), "summary missing `{line}`:\n{}", r.summary);
    }
    assert_eq!(out.matches("suite=").count(), 3, "{out}");
}

#[test]
fn exactly_300_s_passes_and_301_s_fails() {
    let at = report("at300", 90, 300, 0);
    assert!(at.out.status.success(), "300 s is not above 300 s: {}", stdout(&at));
    assert!(stdout(&at).contains("::warning::"));

    let over = report("over", 90, 301, 0);
    assert!(!over.out.status.success(), "301 s must fail");
    let out = stdout(&over);
    assert!(out.contains("::error::"), "{out}");
    assert!(out.contains("suite=suite_sandbox_02 measured=152.20s recorded=152s"), "{out}");
}

#[test]
fn a_failing_test_status_is_passed_through() {
    let r = report("status", 90, 100, 101);
    assert_eq!(r.out.status.code(), Some(101));
}

#[test]
fn ci_yml_times_build_and_run_apart_and_calls_the_report() {
    let ci = std::fs::read_to_string(root().join(".github/workflows/ci.yml")).unwrap();
    let no_run = ci.find("cargo test --no-run $flags").expect("build step in ci.yml");
    let run = ci.find("cargo test $flags -- --nocapture").expect("run step in ci.yml");
    let rep = ci.find("./scripts/ci-shard-report.sh").expect("report call in ci.yml");
    assert!(no_run < run && run < rep, "order: --no-run, run, report");
    assert!(!ci.contains("-gt 300"), "the budget lives in ci-shard-report.sh");
    assert!(
        ci.contains("cargo test --locked $(./scripts/ci-test-partition.sh core)"),
        "the core job's test step must be untouched"
    );
}
