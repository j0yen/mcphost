//! AC6: `scripts/www-check.sh` against a fixture host fetches `/examples`,
//! `/examples/<first>` and `/examples.json`, and exits 1 when `/examples` has
//! zero cards.

use crate::common;
use common::TestServer;
use mcphost::examples::EXAMPLES;
use std::process::{Command, Output};

fn www_check(host: &str) -> Output {
    Command::new("bash")
        .arg(concat!(env!("CARGO_MANIFEST_DIR"), "/scripts/www-check.sh"))
        .env("WWW_CHECK_HOST", host)
        .output()
        .expect("run www-check.sh")
}

#[tokio::test]
async fn exgal_ac06_www_check_passes_on_a_host_with_cards() {
    let server = TestServer::start().await;
    let base = server.base_url.clone();
    let out = tokio::task::spawn_blocking(move || www_check(&base)).await.unwrap();
    assert_eq!(out.status.code(), Some(0), "stderr: {}", String::from_utf8_lossy(&out.stderr));
}

#[tokio::test]
async fn exgal_ac06_www_check_exits_one_when_examples_has_zero_cards() {
    let server = TestServer::start().await;
    // Hide every example: the page renders zero cards.
    let conn = rusqlite::Connection::open(server.state.db.path()).unwrap();
    conn.execute_batch("CREATE TABLE example_proofs (name TEXT PRIMARY KEY, ok INTEGER NOT NULL, at TEXT, log_excerpt TEXT, hidden_reason TEXT);").unwrap();
    for ex in EXAMPLES {
        conn.execute("INSERT INTO example_proofs (name, ok, hidden_reason) VALUES (?1, 0, 'proof failed')", [ex.name]).unwrap();
    }
    let base = server.base_url.clone();
    let out = tokio::task::spawn_blocking(move || www_check(&base)).await.unwrap();
    assert_eq!(out.status.code(), Some(1), "stdout: {}", String::from_utf8_lossy(&out.stdout));
    assert!(String::from_utf8_lossy(&out.stderr).contains("/examples: zero cards"));
}
