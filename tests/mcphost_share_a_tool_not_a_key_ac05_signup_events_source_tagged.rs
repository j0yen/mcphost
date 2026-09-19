//! PRD-mcphost-share-a-tool-not-a-key
//! AC5 — Given the proof completed, When `signup_events` is queried for
//! both tenants, Then `source=recipe:share-a-tool`.
//!
//! `signup_events` has no field literally named `source`; the durable
//! per-signup ledger's equivalent (migration 0012_provenance.sql) is
//! `origin_detail`, the same column `provaudit_ac01_synthetic_signup_stamped.rs`
//! asserts on for an analogous `x-mcphost-synthetic`-header label. `proof.sh`
//! signs up both tenants with `x-mcphost-synthetic: recipe:share-a-tool`,
//! which lands in exactly that column.

use crate::common;
use common::{TestServer, http_kind_registry};
use rusqlite::params;
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
async fn both_tenants_signups_are_tagged_source_recipe_share_a_tool() {
    let server = TestServer::start_with_kinds(http_kind_registry()).await;
    let mcphost_url = format!("{}/mcp", server.base_url);

    let (success, stdout) = run_proof(&mcphost_url).await;
    assert!(
        check_passed(&stdout, "both_tenants_signed_up"),
        "proof.sh did not confirm both tenants signed up:\n{stdout}"
    );
    assert!(success, "proof.sh exited non-zero overall:\n{stdout}");

    let db_path = server.data_dir.0.join("mcphost.db");
    let conn = rusqlite::Connection::open(&db_path).expect("open raw db");
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM signup_events WHERE origin_detail = ?1",
            params!["recipe:share-a-tool"],
            |r| r.get(0),
        )
        .expect("query signup_events");
    assert_eq!(
        count, 2,
        "expected the owner's and the caller's signup_events rows to both carry \
         origin_detail = 'recipe:share-a-tool'"
    );
}
