//! PRD-mcphost-team-memory
//! AC4 — Given the owner removes C from the group, When C calls `recall`,
//! Then a permission error is returned and B's `recall` still succeeds.

use crate::common;
use common::{TempDataDir, TestServer, python_kind_registry};
use mcphost::sandbox;
use tokio::process::Command;

/// See the AC2 test's `run_proof` doc comment for why this must be
/// `tokio::process::Command`, not `std::process::Command`.
async fn run_proof(mcphost_url: &str) -> (bool, String) {
    let script =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/team-memory/proof.sh");
    let output = Command::new("bash")
        .arg(&script)
        .env("MCPHOST_URL", mcphost_url)
        .output()
        .await
        .expect("run examples/team-memory/proof.sh");
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    assert!(!stdout.is_empty(), "proof.sh produced no stdout; stderr:\n{stderr}");
    (output.status.success(), stdout)
}

fn check_passed(stdout: &str, name: &str) -> bool {
    stdout.lines().any(|l| l == format!("CHECK {name}: PASS"))
}

#[tokio::test]
async fn group_remove_revokes_c_while_b_keeps_working() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs_dir = TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let mcphost_url = format!("{}/mcp", server.base_url);

    let (success, stdout) = run_proof(&mcphost_url).await;
    assert!(
        check_passed(&stdout, "owner_group_remove_c"),
        "proof.sh did not confirm the owner removed C from the group:\n{stdout}"
    );
    assert!(
        check_passed(&stdout, "c_recall_after_remove_is_permission_error"),
        "proof.sh did not confirm C's next recall after removal is a permission error:\n{stdout}"
    );
    assert!(
        check_passed(&stdout, "b_recall_still_succeeds_after_c_removed"),
        "proof.sh did not confirm B's recall still succeeds after C's removal:\n{stdout}"
    );
    assert!(success, "proof.sh exited non-zero overall:\n{stdout}");
}
