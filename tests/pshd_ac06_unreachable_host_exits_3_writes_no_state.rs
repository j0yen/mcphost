//! PRD-mcphost-push-from-directory
//! AC6 (P0) — Given the host is unreachable, When `push` runs, Then exit code
//! is 3 within 10 s and no partial state is written to `pushes.jsonl`.

use crate::common;
use common::{TempDataDir, TestServer, ToolDirFixture, tenant_url};
use std::time::Duration;
use tokio::process::Command;

async fn push(dir: &std::path::Path, url: &str, state: &std::path::Path) -> std::process::Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_mcphost"));
    cmd.arg("push").arg(dir).args(["--url", url]).env_remove("MCPHOST_URL").env("XDG_STATE_HOME", state);
    // The 10 s bound is the PRD's: a hung push fails the test here.
    tokio::time::timeout(Duration::from_secs(10), cmd.output())
        .await
        .expect("push returns within 10 s")
        .expect("run mcphost push")
}

#[tokio::test]
async fn pshd_ac06_unreachable_host_exits_3_and_pushes_jsonl_stays_absent() {
    // A port nothing listens on.
    let closed = {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        format!("http://127.0.0.1:{port}/u/secret/mcp")
    };
    let state = TempDataDir::new();
    let dir = ToolDirFixture::from_example("hello");

    let out = push(&dir.0, &closed, &state.0).await;
    assert_eq!(out.status.code(), Some(3), "stdout: {}", String::from_utf8_lossy(&out.stdout));
    assert!(!state.0.join("mcphost/pushes.jsonl").exists(), "an unreachable push writes no state");
    assert!(!String::from_utf8_lossy(&out.stdout).contains("/u/secret"), "the url stays off stdout");

    // Control: a reachable host does record the push, so the absence above is meaningful.
    let server = TestServer::start().await;
    let url = tenant_url(&server.base_url).await;
    let ok = push(&dir.0, &url, &state.0).await;
    assert_eq!(ok.status.code(), Some(0), "stdout: {}", String::from_utf8_lossy(&ok.stdout));
    let rows = std::fs::read_to_string(state.0.join("mcphost/pushes.jsonl")).expect("a published push is recorded");
    let row: serde_json::Value = serde_json::from_str(rows.lines().next().unwrap()).unwrap();
    assert_eq!(row["tool"], "hello");
    assert_eq!(row["version"], 1);
    assert!(row["sha256"].as_str().is_some_and(|s| s.len() == 64) && row["ts"].is_u64(), "{row}");
}
