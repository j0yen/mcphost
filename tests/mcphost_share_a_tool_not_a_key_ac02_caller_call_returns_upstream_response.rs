//! PRD-mcphost-share-a-tool-not-a-key
//! AC2 — Given two fresh tenants, When `proof.sh` runs against
//! `MCPHOST_URL`, Then the caller's `host.tool_call` on the shared tool
//! returns the upstream's response.

use crate::common;
use common::{TestServer, http_kind_registry};
use tokio::process::Command;

/// Runs `examples/share-a-tool/proof.sh` against `mcphost_url`, with no
/// `UPSTREAM_URL` set so the script starts its own local mock upstream
/// (the server under `mcphost_url` must therefore allow a loopback `http`
/// tool, i.e. be built with [`http_kind_registry`]). Returns
/// `(exit_success, stdout)`.
///
/// Uses `tokio::process::Command`, not `std::process::Command`: these
/// tests run under the default (current-thread) `#[tokio::test]` flavor,
/// on the same OS thread the in-process `TestServer`'s axum task is
/// scheduled on. A blocking `std::process::Command::output()` call would
/// starve that thread, so the server could never answer `proof.sh`'s own
/// requests -- a real deadlock, not just a slow test.
async fn run_proof(mcphost_url: &str) -> (bool, String) {
    let script = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("examples/share-a-tool/proof.sh");
    let output = Command::new("bash")
        .arg(&script)
        .env("MCPHOST_URL", mcphost_url)
        .env_remove("UPSTREAM_URL")
        .output()
        .await
        .expect("run examples/share-a-tool/proof.sh");
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    assert!(
        !stdout.is_empty(),
        "proof.sh produced no stdout; stderr:\n{stderr}"
    );
    (output.status.success(), stdout)
}

fn check_passed(stdout: &str, name: &str) -> bool {
    stdout
        .lines()
        .any(|l| l == format!("CHECK {name}: PASS"))
}

#[tokio::test]
async fn callers_tool_call_returns_the_upstreams_response() {
    let server = TestServer::start_with_kinds(http_kind_registry()).await;
    let mcphost_url = format!("{}/mcp", server.base_url);

    let (success, stdout) = run_proof(&mcphost_url).await;
    assert!(
        check_passed(&stdout, "caller_tool_call_returns_upstream_response"),
        "proof.sh did not confirm the caller's call returned the upstream's response:\n{stdout}"
    );
    assert!(success, "proof.sh exited non-zero overall:\n{stdout}");
}
