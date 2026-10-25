//! contract: reads scripts/live/tblurlp_import_live.py
//! PRD-mcphost-table-import-url-live-proof
//! AC3 (P0) -- Given `scripts/live/tblurlp_import_live.py` and the loopback
//! harness app serving its own fixture route, When the driver runs with
//! `--base-url http://127.0.0.1:<port>` (harness signup or `--key-file`), Then
//! its last line starts with `fixture_http=200 rows_imported=1000 count=1000
//! is_error=0` and it exits 0, and `--plan` prints the steps with no network
//! call.

use crate::common::{McpClient, TestServer, extract_structured, signup};
use serde_json::{Value, json};
use std::process::Output;
use tokio::process::Command;

const SCRIPT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/scripts/live/tblurlp_import_live.py");
const PASS_LINE: &str = "fixture_http=200 rows_imported=1000 count=1000 is_error=0";

/// A fresh scratch dir, removed on drop (no `tempfile` crate in this tree).
struct Scratch(std::path::PathBuf);

impl Scratch {
    fn new(tag: &str) -> Scratch {
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let dir = std::env::temp_dir().join(format!("tblurlp-{tag}-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("scratch dir");
        Scratch(dir)
    }

    fn path(&self) -> &std::path::Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

async fn driver(args: &[&str]) -> Output {
    Command::new("python3")
        .arg(SCRIPT)
        .args(args)
        .env("HTTPS_PROXY", "http://127.0.0.1:9")
        .env("HTTP_PROXY", "http://127.0.0.1:9")
        .env("NO_PROXY", "127.0.0.1,localhost")
        .output()
        .await
        .expect("run tblurlp_import_live.py")
}

fn last_line(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).lines().last().unwrap_or_default().to_string()
}

fn rows(path: &std::path::Path) -> Vec<Value> {
    std::fs::read_to_string(path)
        .expect("results file")
        .lines()
        .map(|l| serde_json::from_str(l).expect("results row is JSON"))
        .collect()
}

#[tokio::test]
async fn tblurlp_ac03_signup_run_imports_counts_and_offboards() {
    let server = TestServer::start().await;
    let dir = Scratch::new("signup");
    let results = dir.path().join("results.jsonl");
    let out = driver(&["--base-url", &server.base_url, "--results", results.to_str().unwrap()]).await;
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();

    assert_eq!(out.status.code(), Some(0), "{stdout}\n{stderr}");
    assert_eq!(last_line(&out), format!("{PASS_LINE} offboarded=1"), "{stdout}");
    let rows = rows(&results);
    assert_eq!(rows.len(), 1);
    let row = &rows[0];
    assert_eq!(row["result"], json!("pass"), "{row}");
    assert_eq!(row["summary"], json!(last_line(&out)), "the row's summary is the stdout last line");
    assert_eq!(row["table"], json!("tblurlp_ac04"));
    assert_eq!(row["fixture_http"], json!(200));
    let fixture = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/examples/database-in-a-minute/fixture.csv")).unwrap();
    assert_eq!(row["fixture_bytes"], json!(fixture.len()));
    for field in ["ts", "base_url", "error_code", "elapsed_s"] {
        assert!(row.get(field).is_some(), "{field} missing from {row}");
    }
    assert!(!stdout.contains("\"key\""), "no key on stdout");
}

#[tokio::test]
async fn tblurlp_ac03_key_file_run_skips_signup_and_offboard_and_keeps_the_file() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "TBLURLP AC3 key-file").await;
    let dir = Scratch::new("keyfile");
    let key_file = dir.path().join("tenant.key");
    std::fs::write(&key_file, &key).unwrap();
    let results = dir.path().join("results.jsonl");

    let out = driver(&[
        "--base-url", &server.base_url,
        "--key-file", key_file.to_str().unwrap(),
        "--table", "tblurlp_kf",
        "--results", results.to_str().unwrap(),
    ])
    .await;
    assert_eq!(out.status.code(), Some(0), "{}", String::from_utf8_lossy(&out.stdout));
    assert_eq!(last_line(&out), format!("{PASS_LINE} offboarded=0"));
    assert_eq!(std::fs::read_to_string(&key_file).unwrap(), key, "a --key-file is never deleted");
    assert!(!String::from_utf8_lossy(&out.stdout).contains(&key));
    assert!(!String::from_utf8_lossy(&out.stderr).contains(&key));
    assert!(!std::fs::read_to_string(&results).unwrap().contains(&key), "no key in the results file");

    // The harness tenant was not offboarded: it still counts the imported table.
    let client = McpClient::with_bearer(&server.base_url, &key);
    let count = extract_structured(&client.tools_call("host_table_count", json!({"table": "tblurlp_kf"})).await.expect("count"));
    assert_eq!(count["count"], json!(1000));
}

#[tokio::test]
async fn tblurlp_ac03_an_unserved_fixture_is_exit_3_never_a_fail() {
    let server = TestServer::start().await;
    let dir = Scratch::new("pending");
    let results = dir.path().join("results.jsonl");
    let missing = format!("{}/examples/not-served.csv", server.base_url);
    let out = driver(&["--base-url", &server.base_url, "--source-url", &missing, "--results", results.to_str().unwrap()]).await;
    assert_eq!(out.status.code(), Some(3), "{}", String::from_utf8_lossy(&out.stdout));
    assert_eq!(last_line(&out), "fixture_http=404 rows_imported=0 count=0 is_error=0 offboarded=0");
    assert_eq!(rows(&results)[0]["result"], json!("pending"));
}

#[tokio::test]
async fn tblurlp_ac03_plan_prints_the_steps_with_no_network_call() {
    let out = Command::new("python3")
        .arg(SCRIPT)
        .arg("--plan")
        .env("HTTPS_PROXY", "http://127.0.0.1:9")
        .env("HTTP_PROXY", "http://127.0.0.1:9")
        .env("MCPHOST_BASE_URL", "http://127.0.0.1:9")
        .output()
        .await
        .expect("run --plan");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{stdout} {}", String::from_utf8_lossy(&out.stderr));
    let mut from = 0;
    for step in ["fixture URL", "signup", "host_table_create", "host_table_count", "host_self_offboard"] {
        let at = stdout[from..].find(step).unwrap_or_else(|| panic!("`{step}` missing or out of order in:\n{stdout}"));
        from += at + step.len();
    }
    assert!(stdout.contains("synthorg-tblurlp"));
    assert!(out.stderr.is_empty(), "no connection error: no network call");
}
