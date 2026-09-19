//! PRD-mcphost-share-a-tool-not-a-key
//! AC4 — Given `host.group.remove` for the caller, When the caller calls
//! the tool again, Then the response is a permission error and the
//! owner's own call still succeeds.

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
async fn group_remove_revokes_the_caller_while_the_owner_keeps_working() {
    let server = TestServer::start_with_kinds(http_kind_registry()).await;
    let mcphost_url = format!("{}/mcp", server.base_url);

    let (success, stdout) = run_proof(&mcphost_url).await;
    assert!(
        check_passed(&stdout, "group_remove_causes_permission_error"),
        "proof.sh did not confirm the removed caller's next call is a permission error:\n{stdout}"
    );
    assert!(
        check_passed(&stdout, "owner_call_still_succeeds_after_remove"),
        "proof.sh did not confirm the owner's own call still succeeds after removal:\n{stdout}"
    );
    assert!(success, "proof.sh exited non-zero overall:\n{stdout}");
}
