//! PRD-mcphost-example-proof-runner-contract
//! AC3 — Given a proof that fails, When `examples prove` prints its line,
//! Then the next lines are the last 20 lines of that proof's stdout+stderr
//! indented by two spaces, without `--report`; and Given a passing proof,
//! Then no excerpt is printed.

use mcphost::example_proofs::{EXCERPT_LINES, RunOpts, run};

fn stub(name: &str, body: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("eprc-ac03-{name}-{}-{}", std::process::id(), mcphost::state::now_unix_ms()));
    std::fs::create_dir_all(dir.join(name)).expect("stub dir");
    std::fs::write(dir.join(name).join("proof.sh"), body).expect("stub");
    dir
}

async fn prove(name: &str, body: &str) -> Vec<String> {
    let opts = RunOpts { url: "http://127.0.0.1:1/mcp".to_string(), only: Some(name.to_string()), report: false, dry_run: false, admin_key: None, files_dir: Some(stub(name, body)) };
    let mut out = Vec::new();
    run(&opts, &mut out).await.expect("runner");
    String::from_utf8_lossy(&out).lines().map(str::to_string).collect()
}

#[tokio::test]
async fn eprc_ac03() {
    assert_eq!(EXCERPT_LINES, 20);

    // 25 stdout lines then 2 stderr lines, exit 1: the excerpt is the last 20 of the 27.
    let lines = prove("retrieval", "for i in $(seq 1 25); do echo \"out $i\"; done\necho 'err a' >&2\necho 'err b' >&2\nexit 1\n").await;
    assert!(lines[0].starts_with("retrieval FAIL "), "{lines:?}");
    assert_eq!(lines.len(), 1 + EXCERPT_LINES, "{lines:?}");
    assert!(lines[1..].iter().all(|l| l.starts_with("  ") && !l.starts_with("   ")), "two-space indent: {lines:?}");
    assert_eq!(lines[1], "  out 8");
    assert_eq!(lines[EXCERPT_LINES], "  err b");

    // A passing proof prints its one line and nothing else.
    let lines = prove("uptime-probes", "echo noisy\nexit 0\n").await;
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert!(lines[0].starts_with("uptime-probes PASS "), "{lines:?}");
}
