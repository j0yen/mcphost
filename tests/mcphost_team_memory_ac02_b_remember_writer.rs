//! PRD-mcphost-team-memory
//! AC2 — Given three fresh tenants set up per the section, When tenant B
//! calls `remember`, Then the row exists in the owner's `memory` table
//! with `writer` = B's tenant id (the documented fallback: mcphost does
//! not yet expose the calling tenant's own id inside a shared `python`
//! tool, so `remember` takes a caller-supplied `who` and trusts it --
//! `proof.sh`'s own output states this, see its `WRITER=` line).

use crate::common;
use common::{TempDataDir, TestServer, python_kind_registry};
use mcphost::sandbox;
use tokio::process::Command;

/// Runs `examples/team-memory/proof.sh` against `mcphost_url`. Uses
/// `tokio::process::Command`, not `std::process::Command`: these tests run
/// under the default (current-thread) `#[tokio::test]` flavor, on the same
/// OS thread the in-process `TestServer`'s axum task is scheduled on -- a
/// blocking `std::process::Command::output()` call would starve that
/// thread, so the server could never answer `proof.sh`'s own requests (see
/// `examples/share-a-tool`'s AC2 test for the same reasoning).
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
async fn bs_remember_is_written_with_b_as_writer() {
    // Requirement 8/9: a real python-kind tool through the sandbox needs
    // unprivileged user namespaces, not guaranteed on every runner -- skip
    // cleanly in CI, fail loudly elsewhere (same convention as every other
    // sandbox-dependent test in this crate, e.g. tests/tables_ac05_python_sandbox_table_access.rs).
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs_dir = TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let mcphost_url = format!("{}/mcp", server.base_url);

    let (success, stdout) = run_proof(&mcphost_url).await;
    assert!(
        check_passed(&stdout, "b_remember_succeeds"),
        "proof.sh did not confirm B's remember call succeeded:\n{stdout}"
    );
    assert!(
        check_passed(&stdout, "b_remember_writer_is_b"),
        "proof.sh did not confirm the remembered row's writer is B's own tenant id:\n{stdout}"
    );
    // The one that actually proves AC2's Then: `b_remember_writer_is_b`
    // above only reads `remember`'s own response, and `remember.py` echoes
    // the caller-supplied `who` back verbatim -- so it stays green even if
    // the stored row's `writer` were wrong or the row were never written.
    // This check re-reads the row out of the owner's `memory` table with
    // the owner's key via `host.table.query`.
    assert!(
        check_passed(&stdout, "b_remember_row_in_table_has_writer_b"),
        "proof.sh did not confirm the row stored in the owner's `memory` table \
         carries B's tenant id as `writer`:\n{stdout}"
    );
    assert!(success, "proof.sh exited non-zero overall:\n{stdout}");
}
