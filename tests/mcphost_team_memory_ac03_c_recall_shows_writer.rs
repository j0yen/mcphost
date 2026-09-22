//! PRD-mcphost-team-memory
//! AC3 — Given B's row, When tenant C calls `recall "deploy"`, Then the
//! row is returned newest first with B as writer.

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
async fn cs_recall_returns_bs_row_newest_first_with_b_as_writer() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs_dir = TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let mcphost_url = format!("{}/mcp", server.base_url);

    let (success, stdout) = run_proof(&mcphost_url).await;
    assert!(
        check_passed(&stdout, "c_recall_succeeds"),
        "proof.sh did not confirm C's recall call succeeded:\n{stdout}"
    );
    assert!(
        check_passed(&stdout, "c_recall_returns_both_deploy_rows"),
        "proof.sh did not confirm C's recall returned both the older and newer \
         'deploy' rows:\n{stdout}"
    );
    assert!(
        check_passed(&stdout, "c_recall_shows_b_as_writer"),
        "proof.sh did not confirm C's recall showed B as the newest row's writer:\n{stdout}"
    );
    assert!(
        check_passed(&stdout, "c_recall_older_c_row_is_second"),
        "proof.sh did not confirm the older row (C's) sorted after the newer \
         row (B's), i.e. that recall is genuinely newest-first:\n{stdout}"
    );
    assert!(success, "proof.sh exited non-zero overall:\n{stdout}");
}
