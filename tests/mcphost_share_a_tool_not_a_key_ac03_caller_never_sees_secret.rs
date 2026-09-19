//! PRD-mcphost-share-a-tool-not-a-key
//! AC3 — Given the caller tenant, When it calls `host.secret_list`, Then
//! the list is empty and no response body to the caller contains the
//! secret value.

use crate::common;
use common::{TestServer, http_kind_registry};
use tokio::process::Command;

/// See the AC2 test's `run_proof` doc comment: `tokio::process::Command`
/// (not `std::process::Command`) is required to avoid deadlocking the
/// current-thread `#[tokio::test]` runtime the in-process server also
/// runs on.
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
async fn caller_secret_list_is_empty_and_no_response_leaks_the_secret() {
    let server = TestServer::start_with_kinds(http_kind_registry()).await;
    let mcphost_url = format!("{}/mcp", server.base_url);

    let (success, stdout) = run_proof(&mcphost_url).await;
    assert!(
        check_passed(&stdout, "caller_secret_list_empty"),
        "proof.sh did not confirm the caller's host.secret_list is empty:\n{stdout}"
    );
    assert!(
        check_passed(&stdout, "no_secret_leak_in_caller_responses"),
        "proof.sh did not confirm no caller response leaked the secret value:\n{stdout}"
    );
    assert!(success, "proof.sh exited non-zero overall:\n{stdout}");
}
