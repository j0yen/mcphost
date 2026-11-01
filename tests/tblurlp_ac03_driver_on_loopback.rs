//! contract: reads scripts/live/tblurlp_import_live.py
//! PRD-mcphost-table-import-url-live-proof
//! AC3 (P0) -- Given `scripts/live/tblurlp_import_live.py` and the loopback
//! harness app serving its own fixture route, When the driver runs with
//! `--base-url http://127.0.0.1:<port>` (harness signup or `--key-file`), Then
//! its last line starts with `fixture_http=200 rows_imported=1000 count=1000
//! is_error=0` and it exits 0, and `--plan` prints the steps with no network
//! call.

use crate::common::{self, TestServer};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const SCRIPT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/scripts/live/tblurlp_import_live.py");
const PASS_PREFIX: &str = "fixture_http=200 rows_imported=1000 count=1000 is_error=0";

fn driver(args: Vec<String>) -> Output {
    Command::new("python3")
        .arg(SCRIPT)
        .args(args)
        // A dead proxy: a loopback request must not use it, a stray remote one fails.
        .env("NO_PROXY", "127.0.0.1")
        .env("no_proxy", "127.0.0.1")
        .env("HTTPS_PROXY", "http://127.0.0.1:9")
        .output()
        .expect("run tblurlp_import_live.py")
}

async fn run_driver(args: Vec<String>) -> Output {
    tokio::task::spawn_blocking(move || driver(args)).await.expect("join")
}

fn last_line(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).lines().last().unwrap_or_default().to_string()
}

fn last_row(results: &Path) -> Value {
    let text = std::fs::read_to_string(results).expect("results file");
    serde_json::from_str(text.lines().last().expect("a row")).expect("row is json")
}

/// A fresh empty directory; `TempDataDir` removes it on drop, so the caller keeps the guard.
fn scratch() -> (common::TempDataDir, PathBuf) {
    let guard = common::TempDataDir::new();
    let dir = guard.0.clone();
    std::fs::create_dir_all(&dir).expect("scratch dir");
    (guard, dir)
}

#[test]
fn tblurlp_ac03_plan_prints_the_steps_and_makes_no_network_call() {
    let out = driver(vec!["--plan".into(), "--base-url".into(), "http://127.0.0.1:9".into()]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{stdout} {}", String::from_utf8_lossy(&out.stderr));
    let mut from = 0;
    for step in ["fixture GET", "signup", "host_table_create", "host_table_count", "host_self_offboard"] {
        let at = stdout[from..].find(step).unwrap_or_else(|| panic!("`{step}` missing or out of order in:\n{stdout}"));
        from += at + step.len();
    }
    assert!(stdout.contains("evidence: fixture_http=<n> rows_imported=<n> count=<n> is_error=<n> offboarded=<n>"));
    assert!(out.stderr.is_empty(), "plan mode makes no connection");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tblurlp_ac03_signup_mode_imports_1000_rows_and_offboards() {
    let server = TestServer::start().await;
    let (_guard, dir) = scratch();
    let results = dir.join("results.jsonl");
    let out = run_driver(vec!["--base-url".into(), server.base_url.clone(), "--results".into(), results.display().to_string()]).await;
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    assert_eq!(out.status.code(), Some(0), "stdout: {stdout}\nstderr: {stderr}");
    let line = last_line(&out);
    assert!(line.starts_with(PASS_PREFIX), "{line}");
    assert_eq!(line, format!("{PASS_PREFIX} offboarded=1"));

    let row = last_row(&results);
    assert_eq!(row["result"], "pass", "{row}");
    assert_eq!(row["summary"], line.as_str(), "the row carries the stdout last line");
    assert_eq!(row["table"], "tblurlp_ac04");
    assert_eq!(row["fixture_bytes"], std::fs::metadata(concat!(env!("CARGO_MANIFEST_DIR"), "/examples/database-in-a-minute/fixture.csv")).unwrap().len());
    for field in ["ts", "base_url", "error_code", "result", "elapsed_s"] {
        assert!(row.get(field).is_some(), "{field} missing from {row}");
    }
    assert!(!stdout.contains("Bearer") && !std::fs::read_to_string(&results).unwrap().contains("Bearer"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tblurlp_ac03_key_file_mode_skips_signup_and_never_deletes_the_key() {
    let server = TestServer::start().await;
    let (_ns, key) = common::signup(&server.base_url, "TBLURLP AC3 keyfile").await;
    let (_guard, dir) = scratch();
    let key_file = dir.join("key");
    std::fs::write(&key_file, &key).expect("write key");
    let results = dir.join("results.jsonl");
    let out = run_driver(vec![
        "--base-url".into(),
        server.base_url.clone(),
        "--key-file".into(),
        key_file.display().to_string(),
        "--results".into(),
        results.display().to_string(),
    ])
    .await;
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    assert_eq!(out.status.code(), Some(0), "stdout: {stdout}\nstderr: {}", String::from_utf8_lossy(&out.stderr));
    assert!(last_line(&out).starts_with(PASS_PREFIX), "{}", last_line(&out));
    assert!(last_line(&out).ends_with("offboarded=0"), "a --key-file tenant is not offboarded");
    assert!(key_file.exists(), "the key file is read-only to the driver");
    assert!(!stdout.contains(&key) && !std::fs::read_to_string(&results).unwrap().contains(&key), "the key is never printed or logged");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tblurlp_ac03_unserved_fixture_exits_3_not_a_fail() {
    let server = TestServer::start().await;
    let (_guard, dir) = scratch();
    let results = dir.join("results.jsonl");
    let out = run_driver(vec![
        "--base-url".into(),
        server.base_url.clone(),
        "--source-url".into(),
        format!("{}/examples/no-such-fixture.csv", server.base_url),
        "--results".into(),
        results.display().to_string(),
    ])
    .await;
    assert_eq!(out.status.code(), Some(3), "{}", String::from_utf8_lossy(&out.stdout));
    assert!(last_line(&out).starts_with("fixture_http=404 "), "{}", last_line(&out));
    assert_eq!(last_row(&results)["result"], "pending");
}
