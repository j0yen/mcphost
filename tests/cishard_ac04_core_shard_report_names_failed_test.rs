//! PRD-mcphost-ci-core-suites-sharded AC4 -- given a core shard log with one
//! failing test, `ci-shard-report.sh core` on it with the tests' non-zero
//! status prints `core shard N/M FAILED <test>` for that test and exits
//! non-zero, so `gh run view --log-failed` names the test and the shard.

use std::path::PathBuf;
use std::process::{Command, Output};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// What `cargo test -- --nocapture` writes when one test panics: the test's
/// own stderr lands between `test <name> ... ` and its `FAILED`, so the verdict
/// is NOT on a `... FAILED` line; libtest's closing `failures:` list is the
/// reliable source.
const NOCAPTURE_LOG: &str = "\
     Running tests/suite_core_07.rs (target/debug/deps/suite_core_07-aaaa)

running 3 tests
test alpha::passes_first ... ok
test beta::breaks_the_contract ... thread 'beta::breaks_the_contract' panicked at tests/beta.rs:9:5:
assertion `left == right` failed
  left: 1
 right: 2
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace
FAILED
test gamma::passes_last ... ok

failures:
    beta::breaks_the_contract

test result: FAILED. 2 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 12.50s
";

/// The captured-output shape (no --nocapture): a `---- name stdout ----` block.
const CAPTURED_LOG: &str = "\
     Running tests/suite_core_08.rs (target/debug/deps/suite_core_08-bbbb)

running 2 tests
test delta::passes ... ok
test epsilon::breaks ... FAILED

failures:

---- epsilon::breaks stdout ----
thread 'epsilon::breaks' panicked at tests/epsilon.rs:3:5:
boom


failures:
    epsilon::breaks

test result: FAILED. 1 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 3.10s
";

struct Run {
    out: Output,
    summary: String,
}

fn report(tag: &str, log: &str, status: u32) -> Run {
    let dir = std::env::temp_dir().join(format!("cishard-core-ac04-{}-{tag}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let log_path = dir.join("cargo-test.log");
    let summary = dir.join("summary.md");
    std::fs::write(&log_path, log).unwrap();
    std::fs::write(&summary, "").unwrap();
    let out = Command::new(root().join("scripts/ci-shard-report.sh"))
        .args(["core", "2", "3", "90", "100"])
        .arg(&log_path)
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
fn a_failing_test_is_named_with_its_shard_and_the_status_is_kept() {
    let r = report("nocapture", NOCAPTURE_LOG, 101);
    assert_eq!(r.out.status.code(), Some(101), "{}", stdout(&r));
    let out = stdout(&r);
    assert!(out.contains("core shard 2/3 FAILED beta::breaks_the_contract"), "{out}");
    assert_eq!(out.matches("FAILED").count(), 1, "only the failing test is named: {out}");
    assert!(r.summary.contains("core shard 2/3 FAILED beta::breaks_the_contract"), "{}", r.summary);
}

#[test]
fn the_captured_output_shape_names_the_test_once() {
    let r = report("captured", CAPTURED_LOG, 101);
    assert_eq!(r.out.status.code(), Some(101));
    let out = stdout(&r);
    assert_eq!(out.matches("core shard 2/3 FAILED epsilon::breaks").count(), 1, "{out}");
}

#[test]
fn color_codes_in_the_log_do_not_hide_the_name() {
    let colored = NOCAPTURE_LOG
        .replace("     Running", "\u{1b}[1m\u{1b}[92m     Running\u{1b}[0m")
        .replace("failures:", "\u{1b}[1mfailures:\u{1b}[0m");
    let r = report("color", &colored, 101);
    assert_eq!(r.out.status.code(), Some(101));
    // `failures:` wrapped in color escapes is stripped before matching.
    assert!(stdout(&r).contains("core shard 2/3 FAILED beta::breaks_the_contract"), "{}", stdout(&r));
}

#[test]
fn a_nonzero_status_with_no_parsable_failure_says_so_instead_of_staying_silent() {
    let r = report("cut", "     Running tests/suite_core_07.rs (x)\n\nrunning 9 tests\ntest a::b ... ok\n", 143);
    assert_eq!(r.out.status.code(), Some(143));
    let out = stdout(&r);
    assert!(out.contains("core shard 2/3 ERROR no failed test parsed"), "{out}");
    assert!(!out.contains("core shard 2/3 FAILED"), "{out}");
}

#[test]
fn a_green_log_prints_no_failed_line_and_exits_zero() {
    let r = report("green", "test a::b ... ok\ntest result: ok. 1 passed; 0 failed\n", 0);
    assert!(r.out.status.success(), "{}", stdout(&r));
    assert!(!stdout(&r).contains("FAILED"));
}

#[test]
fn ci_yml_hands_the_shard_report_the_core_partition_and_the_test_status() {
    let ci = std::fs::read_to_string(root().join(".github/workflows/ci.yml")).unwrap();
    assert!(
        ci.contains(r#"./scripts/ci-shard-report.sh core "$shard" "$total" "$build_s" "$run_s" /tmp/cargo-test.log "$status""#),
        "the core shard step must end with the report, handing it the tests' status"
    );
}

#[test]
fn the_shard_steps_survive_a_failing_pipeline_so_the_report_runs() {
    // GitHub runs `run:` under `bash -e`; with pipefail a failing `cargo test | tee`
    // ended the step before `status=$?` and the report (PR #407 runs 2 and 3: exit
    // 101 and no `FAILED <test>` annotation).
    let ci = std::fs::read_to_string(root().join(".github/workflows/ci.yml")).unwrap();
    assert!(
        !ci.contains("tee /tmp/cargo-test.log\n          status=$?"),
        "a bare `cargo test | tee` then `status=$?` dies under bash -e before the report"
    );
    assert_eq!(ci.matches("tee /tmp/cargo-test.log || status=$?").count(), 2, "sandbox and core shard steps");
}
