//! PRD-mcphost-shared-call-run-scope
//! AC8 — Given `examples/share-a-tool/proof.sh` against the in-process test
//! host, When it runs, Then the new owner-ledger step passes.

use crate::common;
use common::{TestServer, http_kind_registry};
use tokio::process::Command;

/// Same invocation shape as `sharedcall_ac06_proof_script_qualified_host_
/// tool_call.rs`'s own `run_proof` -- `tokio::process::Command`, not
/// `std::process::Command`, so the blocking spawn/wait never starves the
/// current-thread runtime the in-process `TestServer`'s own axum task
/// shares with this test.
async fn run_proof(mcphost_url: &str) -> String {
    let script =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/share-a-tool/proof.sh");
    let receipt_path = std::env::temp_dir().join(format!(
        "mcphost-sharedrunscope-ac08-receipt-{}-{}.json",
        std::process::id(),
        mcphost::state::now_unix_ms()
    ));
    let output = Command::new("bash")
        .arg(&script)
        .env("MCPHOST_URL", mcphost_url)
        .env_remove("UPSTREAM_URL")
        .env("SHARE_A_TOOL_RECEIPT", &receipt_path)
        .output()
        .await
        .expect("run examples/share-a-tool/proof.sh");
    std::fs::remove_file(&receipt_path).ok();
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    assert!(!stdout.is_empty(), "proof.sh produced no stdout; stderr:\n{stderr}");
    assert!(output.status.success(), "proof.sh exited non-zero overall:\n{stdout}");
    stdout
}

fn check_passed(stdout: &str, name: &str) -> bool {
    stdout.lines().any(|l| l == format!("CHECK {name}: PASS"))
}

#[tokio::test]
async fn proof_script_owner_ledger_step_passes() {
    let server = TestServer::start_with_kinds(http_kind_registry()).await;
    let mcphost_url = format!("{}/mcp", server.base_url);

    let stdout = run_proof(&mcphost_url).await;
    assert!(
        check_passed(&stdout, "owner_runs_list_excludes_callers_sync_calls"),
        "the new owner-ledger step must pass:\n{stdout}"
    );
}
