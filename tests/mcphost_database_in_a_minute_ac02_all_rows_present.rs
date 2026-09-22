//! PRD-mcphost-database-in-a-minute
//! AC2 — Given the 1,000-row fixture and a fresh tenant, When `proof.sh`
//! runs, Then all rows are present (`host.state.query` count = 1000).

use crate::common;
use common::{TempDataDir, TestServer, python_kind_registry};
use mcphost::sandbox;
use tokio::process::Command;

/// Uses `tokio::process::Command`, not `std::process::Command`: these
/// tests run under the default (current-thread) `#[tokio::test]` flavor,
/// on the same OS thread the in-process `TestServer`'s axum task is
/// scheduled on -- a blocking `std::process::Command::output()` call
/// would starve that thread, so the server could never answer `proof.sh`'s
/// own requests (see `examples/share-a-tool`'s AC2 test for the same
/// reasoning).
async fn run_proof(mcphost_url: &str) -> (bool, String) {
    let script = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("examples/database-in-a-minute/proof.sh");
    let output = Command::new("bash")
        .arg(&script)
        .env("MCPHOST_URL", mcphost_url)
        .output()
        .await
        .expect("run examples/database-in-a-minute/proof.sh");
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    assert!(!stdout.is_empty(), "proof.sh produced no stdout; stderr:\n{stderr}");
    (output.status.success(), stdout)
}

fn check_passed(stdout: &str, name: &str) -> bool {
    stdout.lines().any(|l| l == format!("CHECK {name}: PASS"))
}

#[tokio::test]
async fn all_1000_fixture_rows_are_present_via_host_state_query() {
    // A real python-kind tool through the sandbox needs unprivileged user
    // namespaces -- skip cleanly in CI, fail loudly elsewhere (same
    // convention as every other sandbox-dependent test in this crate).
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs_dir = TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let mcphost_url = format!("{}/mcp", server.base_url);

    let (success, stdout) = run_proof(&mcphost_url).await;
    assert!(
        check_passed(&stdout, "table_create"),
        "proof.sh did not confirm the table was created:\n{stdout}"
    );
    assert!(
        check_passed(&stdout, "batched_insert_five_calls_of_200_succeed"),
        "proof.sh did not confirm the batched insert succeeded:\n{stdout}"
    );
    assert!(
        check_passed(&stdout, "host_state_query_count_is_1000"),
        "proof.sh did not confirm host.state.query returns all 1000 rows:\n{stdout}"
    );
    assert!(success, "proof.sh exited non-zero overall:\n{stdout}");

    // Independent re-check against the raw sqlite file, the same
    // belt-and-suspenders `mcphost_share_a_tool_not_a_key_ac05`'s AC5 test
    // uses: don't just trust proof.sh's own echoed CHECK line, read the
    // actual store.
    let db_path = server.data_dir.0.join("mcphost.db");
    let conn = rusqlite::Connection::open(&db_path).expect("open raw db");
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM tenant_state_rows",
            [],
            |r| r.get(0),
        )
        .expect("query tenant_state_rows");
    assert_eq!(
        count, 1000,
        "expected the expenses table to hold exactly 1000 rows in the raw store"
    );
}
