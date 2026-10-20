//! contract: reads scripts/ci-test-partition.sh
//! PRD-mcphost-ci-sandbox-shards-balanced AC3 -- a sandbox shard job reports
//! `build wall` and `run wall` in the step summary, warns at run wall >= 240 s,
//! fails only above 300 s, and at EVERY level (warn, fail or neither) prints one
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

/// The committed recorded weight of `suite`, as ci-shard-report.sh prints it.
fn recorded(suite: &str) -> u64 {
    let text = std::fs::read_to_string(root().join("scripts/ci-suite-weights.json")).expect("weights file");
    let json: serde_json::Value = serde_json::from_str(&text).expect("weights JSON");
    json[suite].as_u64().unwrap_or_else(|| panic!("{suite} has a recorded weight"))
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
fn under_240_s_raises_no_warning_but_still_reports_every_suite() {
    // A sandbox shard under its warn line used to record nothing, so a suite recorded at
    // 152 s that really ran ~300 s alarmed nothing: every level now prints the suite lines.
    let r = report("quiet", 400, 239, 0);
    assert!(r.out.status.success());
    let out = stdout(&r);
    assert!(!out.contains("::warning::") && !out.contains("::error::"), "{out}");
    for (suite, measured) in [("lib", "5.70"), ("suite_sandbox_02", "152.20"), ("suite_env_sandbox_01", "7.10")] {
        let line = format!("suite={suite} measured={measured}s recorded={}s", recorded(suite));
        let line = line.as_str();
        assert!(out.contains(line), "missing `{line}` in:\n{out}");
        assert!(r.summary.contains(line), "summary missing `{line}`:\n{}", r.summary);
    }
    assert_eq!(out.matches("suite=").count(), 3 + 3, "3 stdout lines + 3 in the readout annotation: {out}");
    assert!(out.contains("::notice title=sandbox shard 2/3 readout::"), "{out}");
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
    for (suite, measured) in [("lib", "5.70"), ("suite_sandbox_02", "152.20"), ("suite_env_sandbox_01", "7.10")] {
        let line = format!("suite={suite} measured={measured}s recorded={}s", recorded(suite));
        let line = line.as_str();
        assert!(out.contains(line), "missing `{line}` in:\n{out}");
        assert!(r.summary.contains(line), "summary missing `{line}`:\n{}", r.summary);
    }
    assert_eq!(out.matches("suite=").count(), 3 + 3, "3 stdout lines + 3 in the readout annotation: {out}");
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
    let line = format!("suite=suite_sandbox_02 measured=152.20s recorded={}s", recorded("suite_sandbox_02"));
    assert!(out.contains(&line), "{out}");
}

#[test]
fn a_failing_test_status_is_passed_through() {
    let r = report("status", 90, 100, 101);
    assert_eq!(r.out.status.code(), Some(101));
}

#[test]
fn ci_yml_times_build_and_run_apart_and_calls_the_report() {
    let ci = std::fs::read_to_string(root().join(".github/workflows/ci.yml")).unwrap();
    // Each shard job (sandbox, and since PRD-mcphost-ci-core-suites-sharded
    // core) times the build and the run apart and ends with the report.
    for (job, no_run, run) in [
        ("\n  sandbox:\n", "cargo test --no-run $flags", "cargo test $flags -- --nocapture"),
        ("\n  core:\n", "cargo test --locked --no-run $flags", "cargo test --locked $flags -- --nocapture"),
    ] {
        let start = ci.find(job).unwrap_or_else(|| panic!("job {job:?} in ci.yml"));
        let body = &ci[start + 1..];
        let end = body.find("\n  gate:\n").or_else(|| body.find("\n  sandbox-required:\n")).unwrap_or(body.len());
        let body = &body[..end];
        let no_run_at = body.find(no_run).unwrap_or_else(|| panic!("build step in {job:?}"));
        let run_at = body.find(run).unwrap_or_else(|| panic!("run step in {job:?}"));
        let rep = body.find("./scripts/ci-shard-report.sh").unwrap_or_else(|| panic!("report call in {job:?}"));
        assert!(no_run_at < run_at && run_at < rep, "order in {job:?}: --no-run, run, report");
    }
    assert!(!ci.contains("-gt 300"), "the budget lives in ci-shard-report.sh");
    assert!(
        !ci.contains("-gt 720") && !ci.contains("-gt 540") && !ci.contains("-gt 420"),
        "the core budget lives in ci-shard-report.sh too"
    );
    // The core partition runs as shards now (PRD-mcphost-ci-core-suites-sharded):
    // the single `$(ci-test-partition.sh core)` step is gone.
    assert!(
        !ci.contains("cargo test --locked $(./scripts/ci-test-partition.sh core)"),
        "the unsharded core test step must be gone"
    );
    assert!(
        ci.contains(r#"flags=$(./scripts/ci-test-partition.sh core-shard "$shard" "$total")"#),
        "the core shard step derives its flags from core-shard"
    );
}

// PRD-mcphost-ci-core-suites-sharded AC6 (readout half): the core shards use the
// same script with the `core` partition, the warn and fail lines printed by
// `ci-test-partition.sh limits core` (the one definition), and always print one
// `suite=` line per suite.
const CORE_LOG: &str = "\
     Running unittests src/main.rs (target/debug/deps/mcphost-aaaa)
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
     Running tests/suite_core_07.rs (target/debug/deps/suite_core_07-bbbb)
test result: ok. 1225 passed; 0 failed; 5 ignored; 0 measured; 0 filtered out; finished in 341.50s
     Running tests/suite_contracts_core_01.rs (target/debug/deps/suite_contracts_core_01-cccc)
test result: ok. 85 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 60.10s
";

fn core_report(tag: &str, run_s: u32) -> (Output, String) {
    let dir = std::env::temp_dir().join(format!("cishard-core-ac03r-{}-{tag}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let log = dir.join("cargo-test.log");
    let summary = dir.join("summary.md");
    std::fs::write(&log, CORE_LOG).unwrap();
    std::fs::write(&summary, "").unwrap();
    let o = Command::new(root().join("scripts/ci-shard-report.sh"))
        .args(["core", "1", "3", "75", &run_s.to_string()])
        .arg(&log)
        .arg("0")
        .env("GITHUB_STEP_SUMMARY", &summary)
        .current_dir(root())
        .output()
        .unwrap();
    let s = std::fs::read_to_string(&summary).unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    (o, s)
}

#[test]
fn a_core_shard_always_reports_build_run_and_every_suite_to_the_summary() {
    let (o, summary) = core_report("quiet", 300);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stdout));
    let so = String::from_utf8_lossy(&o.stdout).into_owned();
    assert!(!so.contains("::warning::") && !so.contains("::error::"), "{so}");
    for line in [
        "core shard 1/3 build wall: 75s",
        "core shard 1/3 run wall: 300s",
        "suite=suite_core_07 measured=341.50s recorded=",
        "suite=suite_contracts_core_01 measured=60.10s recorded=",
        "suite=bin:src/main.rs measured=0.00s recorded=unrecorded",
    ] {
        assert!(summary.contains(line), "summary missing `{line}`:\n{summary}");
    }
}

/// The core run-wall lines `(warn_at, fail_above)` from `ci-test-partition.sh limits core`,
/// so this test follows the one definition instead of restating its seconds.
fn core_limits() -> (u32, u32) {
    let o = Command::new(root().join("scripts/ci-test-partition.sh"))
        .args(["limits", "core"])
        .current_dir(root())
        .output()
        .expect("run ci-test-partition.sh limits core");
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let text = String::from_utf8_lossy(&o.stdout).into_owned();
    let v: Vec<u32> = text.split_whitespace().map(|n| n.parse().expect("limits core prints integers")).collect();
    (v[0], v[1])
}

#[test]
fn warns_at_the_warn_line_passes_through_the_fail_line_and_fails_above_it() {
    let (warn, fail) = core_limits();
    assert!(warn < fail, "warn line {warn} must sit below fail line {fail}");
    let (below, _) = core_report("below", warn - 1);
    assert!(!String::from_utf8_lossy(&below.stdout).contains("::warning::"));
    let (at, _) = core_report("at", warn);
    assert!(at.status.success());
    assert!(String::from_utf8_lossy(&at.stdout).contains("::warning::"));
    let (top, _) = core_report("top", fail);
    assert!(top.status.success(), "{fail} s is not above {fail} s");
    let (over, _) = core_report("over", fail + 1);
    assert!(!over.status.success(), "{} s must fail", fail + 1);
    assert!(String::from_utf8_lossy(&over.stdout).contains("::error::"));
}
