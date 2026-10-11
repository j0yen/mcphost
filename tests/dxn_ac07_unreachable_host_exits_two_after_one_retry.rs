//! PRD-mcphost-docs-examples-executed-nightly AC7 (P1) -- Given the fixture
//! host unreachable, When `run --url` is pointed at a closed port, Then it
//! exits 2 with `host unreachable` after one retry and writes no artifact.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use tokio::net::TcpListener;
use tokio::process::Command;

fn corpus_root() -> PathBuf {
    let root = std::env::temp_dir().join(format!("dxn-ac07-{}-{}", std::process::id(), mcphost::state::now_unix_ms()));
    std::fs::create_dir_all(root.join("docs")).unwrap();
    std::fs::write(
        root.join("docs/page.md"),
        "# P\n\n## One\n\n```json mcphost-call\n{\"name\": \"host.whoami\", \"arguments\": {}}\n```\n",
    )
    .unwrap();
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_mcphost"))
        .args(["docs-examples", "extract", "--root"])
        .arg(&root)
        .output()
        .unwrap();
    assert!(out.status.success());
    root
}

async fn run(root: &std::path::Path, url: &str) -> (Option<i32>, String, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_mcphost"))
        .args(["docs-examples", "run", "--url", url, "--root"])
        .arg(root)
        .arg("--artifact-dir")
        .arg(root.join("artifacts"))
        .env_remove("MCPHOST_TENANT_KEY")
        .output()
        .await
        .unwrap();
    (out.status.code(), String::from_utf8_lossy(&out.stdout).to_string(), String::from_utf8_lossy(&out.stderr).to_string())
}

#[tokio::test]
async fn closed_port_exits_two_with_host_unreachable_and_no_artifact() {
    let root = corpus_root();
    // Bind to learn a free port, then close it again.
    let port = {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        l.local_addr().unwrap().port()
    };
    let (code, stdout, stderr) = run(&root, &format!("http://127.0.0.1:{port}")).await;
    assert_eq!(code, Some(2), "stdout={stdout} stderr={stderr}");
    assert!(stderr.contains("host unreachable"), "stderr: {stderr}");
    assert!(!root.join("artifacts").exists(), "no artifact may be written for an unreachable host");
    std::fs::remove_dir_all(&root).ok();
}

#[tokio::test]
async fn the_host_is_tried_exactly_twice() {
    let root = corpus_root();
    // A host that accepts and hangs up: every attempt is a transport failure
    // we can count.
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let attempts = Arc::new(AtomicUsize::new(0));
    let counter = attempts.clone();
    tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else { break };
            counter.fetch_add(1, Ordering::SeqCst);
            drop(stream);
        }
    });
    let (code, _, stderr) = run(&root, &format!("http://127.0.0.1:{port}")).await;
    assert_eq!(code, Some(2), "stderr: {stderr}");
    assert!(stderr.contains("host unreachable"), "stderr: {stderr}");
    assert_eq!(attempts.load(Ordering::SeqCst), 2, "one attempt plus exactly one retry");
    assert!(!root.join("artifacts").exists());
    std::fs::remove_dir_all(&root).ok();
}
