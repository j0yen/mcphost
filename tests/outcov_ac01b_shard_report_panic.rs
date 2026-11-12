//! PRD-mcphost-outsch-ac01-coverage-diagnosable AC1 (shard-report half) --
//! given a `--nocapture`-format log and a `---- <test> stdout ----` log,
//! `scripts/ci-shard-report.sh` prints `FAILED <test>` followed by the first
//! panic message, also in `$GITHUB_STEP_SUMMARY`, cut to 2 000 bytes; with no
//! panic text the FAILED line stands alone.

use std::path::PathBuf;
use std::process::Command;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

struct Run {
    stdout: String,
    summary: String,
    status: Option<i32>,
}

/// Runs the report over `log_text` as a failed core shard (test status 101).
fn report(tag: &str, log_text: &str) -> Run {
    let dir = std::env::temp_dir().join(format!("outcov-ac01b-{}-{tag}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let log = dir.join("cargo-test.log");
    let summary = dir.join("summary.md");
    std::fs::write(&log, log_text).unwrap();
    std::fs::write(&summary, "").unwrap();
    let out = Command::new(root().join("scripts/ci-shard-report.sh"))
        .args(["core", "4", "5", "90", "100"])
        .arg(&log)
        .arg("101")
        .env("GITHUB_STEP_SUMMARY", &summary)
        .current_dir(root())
        .output()
        .expect("run scripts/ci-shard-report.sh");
    let run = Run {
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        summary: std::fs::read_to_string(&summary).unwrap(),
        status: out.status.code(),
    };
    let _ = std::fs::remove_dir_all(&dir);
    run
}

fn fixture(name: &str) -> String {
    let path = root().join("tests/fixtures/ci-logs").join(name);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// The lines after the `FAILED <test>` line, up to the next line that is not
/// a four-space-indented message line.
fn message_after(text: &str, failed_suffix: &str) -> Vec<String> {
    let mut lines = text.lines().skip_while(|l| !l.ends_with(failed_suffix) || !l.contains("FAILED"));
    assert!(lines.next().is_some(), "no FAILED {failed_suffix} line in:\n{text}");
    lines.take_while(|l| l.starts_with("    ")).map(str::to_string).collect()
}

#[test]
fn outcov_ac01b_nocapture_log_names_the_first_panic_message() {
    let run = report("nocapture", &fixture("nocapture.log"));
    assert_eq!(run.status, Some(101), "the tests' own status is the exit code");
    let test = "outsch_ac01_oneof_budget_falsifier::the_corpus_covers";
    for (where_, text) in [("log", &run.stdout), ("summary", &run.summary)] {
        let message = message_after(text, test);
        assert_eq!(message.len(), 1, "{where_}: {message:?}");
        assert_eq!(message[0], "    coverage 192/264 < 80 %: [\"host.queue.create\", \"host.queue.delete\"]", "{where_}");
        assert!(!text.contains("RUST_BACKTRACE"), "{where_}: the note after the message is not part of it");
    }
}

#[test]
fn outcov_ac01b_stdout_block_log_names_the_first_panic_message() {
    let run = report("captured", &fixture("captured-stdout.log"));
    for (where_, text) in [("log", &run.stdout), ("summary", &run.summary)] {
        let message = message_after(text, "epsilon::breaks");
        assert_eq!(
            message,
            ["    coverage 192/264 < 80 %: [\"host.queue.create\"]", "    second line of the message"],
            "{where_}"
        );
    }
}

#[test]
fn outcov_ac01b_a_failure_with_no_panic_text_stands_alone() {
    let run = report("nopanic", &fixture("no-panic.log"));
    for (where_, text) in [("log", &run.stdout), ("summary", &run.summary)] {
        assert!(message_after(text, "eta::killed_by_signal").is_empty(), "{where_}: {text}");
    }
    assert!(run.summary.contains("core shard 4/5 FAILED eta::killed_by_signal"), "{}", run.summary);
}

#[test]
fn outcov_ac01b_a_long_message_is_cut_to_two_thousand_bytes() {
    let long = "x".repeat(5000);
    let log = format!(
        "running 1 test\ntest big::breaks ... thread 'big::breaks' panicked at tests/big.rs:1:1:\n{long}\nnote: run with `RUST_BACKTRACE=1`\nFAILED\n\nfailures:\n    big::breaks\n\ntest result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.00s\n"
    );
    let run = report("long", &log);
    for (where_, text) in [("log", &run.stdout), ("summary", &run.summary)] {
        let message = message_after(text, "big::breaks");
        let bytes: usize = message.iter().map(|l| l.trim_start().len()).sum();
        assert_eq!(bytes, 2000, "{where_}: the message is cut to 2 000 bytes: {bytes}");
    }
}
