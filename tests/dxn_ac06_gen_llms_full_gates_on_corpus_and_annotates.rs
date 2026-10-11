//! PRD-mcphost-docs-examples-executed-nightly AC6 (P0) -- Given a stale
//! corpus, When `scripts/gen-llms-full.sh` runs, Then it exits 1 before
//! writing and names `docs/examples.corpus.json`; Given a fresh corpus and
//! a `run --json` artifact, Then each tagged fence in `www/llms-full.txt`
//! is followed by its `<!-- executed: ... -->` comment.
//!
//! Runs the REAL script against a miniature repo laid out in a temp dir
//! (the script `cd`s to its own parent), with the real `mcphost` binary and
//! a real `run` against the in-process fixture host producing the artifact.

use std::path::{Path, PathBuf};

use crate::common::TestServer;
use tokio::process::Command;

const KIND_PAGE: &str = "Echo kind.\n\n## Publish\n\n```json mcphost-tool kind=echo name=e1\n{\"schema\": {\"type\": \"object\"}}\n```\n\n```json\n{\"untagged\": true}\n```\n";

fn mini_repo() -> PathBuf {
    let root = std::env::temp_dir().join(format!("dxn-ac06-{}-{}", std::process::id(), mcphost::state::now_unix_ms()));
    for d in ["scripts", "docs/kinds", "www"] {
        std::fs::create_dir_all(root.join(d)).unwrap();
    }
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("scripts/gen-llms-full.sh");
    std::fs::copy(script, root.join("scripts/gen-llms-full.sh")).unwrap();
    // README quotes the same tagged fence (like the generated quickstart block).
    std::fs::write(root.join("README.md"), "# Readme\n\n## Quoted\n\n```json mcphost-tool kind=echo name=e1\n{\"schema\": {\"type\": \"object\"}}\n```\n").unwrap();
    std::fs::write(root.join("docs/plans.md"), "plans\n").unwrap();
    std::fs::write(root.join("docs/kinds/echo.md"), KIND_PAGE).unwrap();
    root
}

async fn sh(root: &Path, script_args: &[&str]) -> (Option<i32>, String) {
    let out = Command::new("bash")
        .arg(root.join("scripts/gen-llms-full.sh"))
        .args(script_args)
        .env("MCPHOST_BIN", env!("CARGO_BIN_EXE_mcphost"))
        .env_remove("DOCS_EXAMPLES_ARTIFACT")
        .output()
        .await
        .expect("run gen-llms-full.sh");
    (out.status.code(), String::from_utf8_lossy(&out.stderr).to_string())
}

fn docs_examples_cmd(root: &Path) -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_mcphost"));
    c.arg("docs-examples").env_remove("MCPHOST_TENANT_KEY").current_dir(root);
    c
}

#[tokio::test]
async fn stale_corpus_blocks_rendering_and_a_run_artifact_adds_executed_comments() {
    let server = TestServer::start().await;
    let root = mini_repo();

    // Stale: tagged fences exist, the committed corpus is empty.
    std::fs::write(root.join("docs/examples.corpus.json"), "[]\n").unwrap();
    let (code, stderr) = sh(&root, &[]).await;
    assert_eq!(code, Some(1), "stale corpus must fail: {stderr}");
    assert!(stderr.contains("docs/examples.corpus.json"), "must name the corpus: {stderr}");
    assert!(!root.join("www/llms-full.txt").exists(), "nothing may be written when the corpus is stale");
    let (code, stderr) = sh(&root, &["--check"]).await;
    assert_eq!(code, Some(1), "--check too: {stderr}");

    // Fresh corpus, no artifact: plain render, no comments.
    let out = docs_examples_cmd(&root).arg("extract").output().await.unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let (code, stderr) = sh(&root, &[]).await;
    assert_eq!(code, Some(0), "{stderr}");
    let plain = std::fs::read_to_string(root.join("www/llms-full.txt")).unwrap();
    assert!(!plain.contains("<!-- executed:"), "no artifact, no comments:\n{plain}");
    let (code, stderr) = sh(&root, &["--check"]).await;
    assert_eq!(code, Some(0), "plain render passes --check: {stderr}");

    // Run against the fixture host -> artifact -> comments after tagged fences only.
    let out = docs_examples_cmd(&root)
        .args(["run", "--json", "--url", &server.base_url])
        .output()
        .await
        .unwrap();
    assert_eq!(out.status.code(), Some(0), "run: {}", String::from_utf8_lossy(&out.stderr));
    let report: serde_json::Value = serde_json::from_slice(&out.stdout).expect("run --json prints the report");
    let date = report["date"].as_str().unwrap().to_string();
    assert!(root.join(format!("target/docs-examples/{date}.json")).exists(), "artifact written under target/docs-examples");

    let (code, stderr) = sh(&root, &[]).await;
    assert_eq!(code, Some(0), "{stderr}");
    let full = std::fs::read_to_string(root.join("www/llms-full.txt")).unwrap();
    let comment = format!("<!-- executed: docs/kinds/echo.md#publish-1 pass {date} -->");
    let tagged_fence_end = "{\"schema\": {\"type\": \"object\"}}\n```\n";
    assert!(
        full.contains(&format!("## Kind: echo\n\nEcho kind.\n\n## Publish\n\n```json mcphost-tool kind=echo name=e1\n{tagged_fence_end}{comment}\n")),
        "the kind page's tagged fence is followed by its comment:\n{full}"
    );
    assert!(
        full.contains(&format!("## README\n\n# Readme\n\n## Quoted\n\n```json mcphost-tool kind=echo name=e1\n{tagged_fence_end}{comment}\n")),
        "the README's quoted copy is matched by body:\n{full}"
    );
    assert_eq!(full.matches("<!-- executed:").count(), 2, "untagged fences get no comment:\n{full}");
    assert!(full.contains("```json\n{\"untagged\": true}\n```\n\n"), "untagged fence untouched:\n{full}");

    // --check ignores artifacts: the committed (plain) render is what it compares.
    std::fs::write(root.join("www/llms-full.txt"), &plain).unwrap();
    let (code, stderr) = sh(&root, &["--check"]).await;
    assert_eq!(code, Some(0), "--check must not read the artifact: {stderr}");
    std::fs::remove_dir_all(&root).ok();
}
