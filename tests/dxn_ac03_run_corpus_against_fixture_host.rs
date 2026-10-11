//! PRD-mcphost-docs-examples-executed-nightly AC3 (P0) -- Given the corpus
//! and the fixture host, When `run` executes, Then every entry prints
//! `<id> pass <ms>` and the summary shows `examples_failed=0`; Given one
//! fence changed to `expect=error:unknown_kind` on a passing call, Then
//! that id prints `fail` with the actual code and the exit status is 1.
//!
//! R7's artifact (`<artifact-dir>/<date>.json`) is asserted here too, since
//! it is written by the same `run`.

use std::path::{Path, PathBuf};

use crate::common::TestServer;
use tokio::process::Command;

fn temp_root(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("dxn-ac03-{tag}-{}-{}", std::process::id(), mcphost::state::now_unix_ms()));
    std::fs::create_dir_all(dir.join("docs")).unwrap();
    dir
}

const PAGE: &str = "# Echo\n\n## Publish and call\n\n```json mcphost-tool kind=echo name=e1\n{\"schema\": {\"type\": \"object\", \"properties\": {\"msg\": {\"type\": \"string\"}}, \"required\": [\"msg\"]}}\n```\n\n```json mcphost-call expect=contains:hello\n{\"name\": \"host.tool_test\", \"arguments\": {\"name\": \"e1\", \"args\": {\"msg\": \"hello\"}}}\n```\n\n```\nhost.whoami()\n```\n\n```json mcphost-call\nhost.whoami()\n```\n";

async fn cli(root: &Path, args: &[&str]) -> (Option<i32>, String, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_mcphost"))
        .arg("docs-examples")
        .args(args)
        .arg("--root")
        .arg(root)
        .env_remove("MCPHOST_TENANT_KEY")
        .output()
        .await
        .expect("run mcphost docs-examples");
    (out.status.code(), String::from_utf8_lossy(&out.stdout).to_string(), String::from_utf8_lossy(&out.stderr).to_string())
}

#[tokio::test]
async fn run_passes_every_entry_then_fails_a_changed_expectation_with_the_actual_code() {
    let server = TestServer::start().await;
    let root = temp_root("run");
    std::fs::write(root.join("docs/page.md"), PAGE).unwrap();
    let (code, _, err) = cli(&root, &["extract"]).await;
    assert_eq!(code, Some(0), "extract: {err}");

    let artifacts = root.join("artifacts");
    let (code, stdout, stderr) =
        cli(&root, &["run", "--url", &server.base_url, "--artifact-dir", artifacts.to_str().unwrap()]).await;
    assert_eq!(code, Some(0), "run should pass:\n{stdout}\n{stderr}");
    for id in ["docs/page.md#publish-and-call-1", "docs/page.md#publish-and-call-2", "docs/page.md#publish-and-call-3"] {
        let line = stdout.lines().find(|l| l.starts_with(id)).unwrap_or_else(|| panic!("no line for {id}:\n{stdout}"));
        let rest: Vec<&str> = line[id.len()..].split_whitespace().collect();
        assert_eq!(rest[0], "pass", "{line}");
        rest[1].parse::<u64>().unwrap_or_else(|_| panic!("`{line}` must end in a millisecond count"));
    }
    assert!(stdout.contains("examples_total=3 examples_failed=0 skipped=0"), "summary:\n{stdout}");

    // R7: the run leaves a date-named artifact with the same counts.
    let artifact = std::fs::read_dir(&artifacts).unwrap().next().expect("artifact written").unwrap().path();
    let report: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(artifact).unwrap()).unwrap();
    assert_eq!(report["examples_total"], 3);
    assert_eq!(report["examples_failed"], 0);

    // One passing call, expectation changed to an error it does not raise.
    let edited = PAGE.replace("expect=contains:hello", "expect=error:unknown_kind");
    std::fs::write(root.join("docs/page.md"), edited).unwrap();
    let (code, _, err) = cli(&root, &["extract"]).await;
    assert_eq!(code, Some(0), "extract: {err}");
    let (code, stdout, stderr) =
        cli(&root, &["run", "--url", &server.base_url, "--artifact-dir", artifacts.to_str().unwrap()]).await;
    assert_eq!(code, Some(1), "run should fail:\n{stdout}\n{stderr}");
    let line = stdout.lines().find(|l| l.starts_with("docs/page.md#publish-and-call-2")).expect("line for -2");
    assert!(line.contains(" fail "), "{line}");
    assert!(line.contains("expected=error:unknown_kind") && line.contains("actual=ok"), "{line}");
    assert!(stdout.contains("examples_failed=1"), "summary:\n{stdout}");
    std::fs::remove_dir_all(&root).ok();
}
