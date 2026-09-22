//! PRD-mcphost-database-in-a-minute
//! AC5 — Given the proof completed, When `signup_events` is read, Then
//! `source=recipe:database-in-a-minute`.
//!
//! Same shape as `tests/mcphost_share_a_tool_not_a_key_ac05_signup_events_source_tagged.rs`
//! and `tests/mcphost_team_memory_ac0*`'s equivalent: `signup_events` has no
//! field literally named `source`; the durable per-signup ledger's
//! equivalent (migration 0012_provenance.sql) is `origin_detail`, which
//! `proof.sh` stamps by signing up with `x-mcphost-synthetic:
//! recipe:database-in-a-minute`.

use crate::common;
use common::{TempDataDir, TestServer, python_kind_registry};
use rusqlite::params;
use tokio::process::Command;

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
async fn owner_signup_is_tagged_source_recipe_database_in_a_minute() {
    if mcphost::sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", mcphost::sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs_dir = TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let mcphost_url = format!("{}/mcp", server.base_url);

    let (success, stdout) = run_proof(&mcphost_url).await;
    assert!(
        check_passed(&stdout, "owner_signed_up"),
        "proof.sh did not confirm the owner tenant signed up:\n{stdout}"
    );
    assert!(success, "proof.sh exited non-zero overall:\n{stdout}");

    let db_path = server.data_dir.0.join("mcphost.db");
    let conn = rusqlite::Connection::open(&db_path).expect("open raw db");
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM signup_events WHERE origin_detail = ?1",
            params!["recipe:database-in-a-minute"],
            |r| r.get(0),
        )
        .expect("query signup_events");
    assert_eq!(
        count, 1,
        "expected the owner's signup_events row to carry \
         origin_detail = 'recipe:database-in-a-minute'"
    );
}
