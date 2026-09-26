//! PRD-mcphost-shared-tool-call-path
//! AC6 — Given examples/share-a-tool/proof.sh against the in-process test
//! host, When it runs, Then both the raw and the host.tool_call qualified
//! calls succeed and the receipt records both.

use crate::common;
use common::{TestServer, http_kind_registry};
use serde_json::Value;
use tokio::process::Command;

/// Runs `examples/share-a-tool/proof.sh` against `mcphost_url`, with no
/// `UPSTREAM_URL` set so the script starts its own local mock upstream, and
/// `SHARE_A_TOOL_RECEIPT` pointed at a scratch path this test owns (the
/// script's own default writes under the OS temp dir keyed by its own pid,
/// but pinning it here means this test knows exactly where to read it back
/// from). Uses `tokio::process::Command`, not `std::process::Command`: this
/// test runs under the default (current-thread) `#[tokio::test]` flavor, on
/// the same OS thread the in-process `TestServer`'s axum task is scheduled
/// on -- a blocking `Command::output()` call would starve that thread, so
/// the server could never answer proof.sh's own requests.
async fn run_proof(mcphost_url: &str, receipt_path: &std::path::Path) -> (bool, String) {
    let script =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/share-a-tool/proof.sh");
    let output = Command::new("bash")
        .arg(&script)
        .env("MCPHOST_URL", mcphost_url)
        .env_remove("UPSTREAM_URL")
        .env("SHARE_A_TOOL_RECEIPT", receipt_path)
        .output()
        .await
        .expect("run examples/share-a-tool/proof.sh");
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    assert!(!stdout.is_empty(), "proof.sh produced no stdout; stderr:\n{stderr}");
    (output.status.success(), stdout)
}

fn check_passed(stdout: &str, name: &str) -> bool {
    stdout.lines().any(|l| l == format!("CHECK {name}: PASS"))
}

#[tokio::test]
async fn proof_script_exercises_both_raw_and_host_tool_call_qualified_forms() {
    let server = TestServer::start_with_kinds(http_kind_registry()).await;
    let mcphost_url = format!("{}/mcp", server.base_url);
    let receipt_path = std::env::temp_dir().join(format!(
        "mcphost-sharedcall-ac06-receipt-{}-{}.json",
        std::process::id(),
        mcphost::state::now_unix_ms()
    ));

    let (success, stdout) = run_proof(&mcphost_url, &receipt_path).await;
    assert!(success, "proof.sh exited non-zero overall:\n{stdout}");
    assert!(
        check_passed(&stdout, "caller_tool_call_returns_upstream_response"),
        "raw tools/call qualified-name step did not pass:\n{stdout}"
    );
    assert!(
        check_passed(&stdout, "caller_host_tool_call_returns_upstream_response"),
        "host.tool_call qualified-name step did not pass:\n{stdout}"
    );

    let receipt_text = std::fs::read_to_string(&receipt_path)
        .unwrap_or_else(|e| panic!("read receipt at {}: {e}\nstdout:\n{stdout}", receipt_path.display()));
    let receipt: Value = serde_json::from_str(&receipt_text).expect("receipt must be valid JSON");
    assert_eq!(
        receipt["raw_tools_call_qualified_ok"],
        Value::Bool(true),
        "receipt must record the raw qualified call succeeding: {receipt}"
    );
    assert_eq!(
        receipt["host_tool_call_qualified_ok"],
        Value::Bool(true),
        "receipt must record the host.tool_call qualified call succeeding: {receipt}"
    );
    assert!(receipt["wall_time_ms"].as_i64().is_some_and(|ms| ms >= 0));

    std::fs::remove_file(&receipt_path).ok();
}
