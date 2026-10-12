//! PRD-mcphost-feedback-tool-and-ledger AC7 — Given 3 rows on the host DB,
//! When `mcphost feedback list --since 24h --kind bug` runs, Then it prints
//! only the `bug` rows and the output contains no `ip_hash`.

use crate::common;
use common::TempDataDir;
use mcphost::db::{Db, FeedbackCap};
use std::process::Command;

async fn put(db: &Db, id: &str, kind: &str, text: &str, created: i64) {
    db.insert_feedback(
        id.to_string(),
        None,
        kind.to_string(),
        text.to_string(),
        None,
        Some("claude-code".to_string()),
        Some("2.1".to_string()),
        "[]".to_string(),
        "SECRET-IP-HASH-VALUE".to_string(),
        created,
        FeedbackCap::None,
    )
    .await
    .expect("insert")
    .expect("admitted");
}

#[tokio::test]
async fn cli_list_prints_only_the_requested_kind_and_never_the_ip_hash() {
    let dir = TempDataDir::new();
    {
        let db = Db::open(&dir.0).expect("open db");
        db.migrate().await.expect("migrate");
        let now = mcphost::state::now_unix();
        put(&db, "f_00000000000000a1", "bug", "bug-one-text", now - 60).await;
        put(&db, "f_00000000000000a2", "praise", "praise-text", now - 30).await;
        put(&db, "f_00000000000000a3", "bug", "bug-old-text", now - 3 * 86_400).await;
    }
    let out = Command::new(env!("CARGO_BIN_EXE_mcphost"))
        .args(["feedback", "list", "--since", "24h", "--kind", "bug"])
        .env("MCPHOST_DATA_DIR", &dir.0)
        .output()
        .expect("run mcphost");
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("bug-one-text"), "{stdout}");
    assert!(!stdout.contains("praise-text"), "other kinds are filtered: {stdout}");
    assert!(!stdout.contains("bug-old-text"), "--since 24h drops older rows: {stdout}");
    assert_eq!(stdout.lines().count(), 2, "header plus the one bug row: {stdout}");
    assert!(!stdout.contains("SECRET-IP-HASH-VALUE") && !stdout.contains("ip_hash"), "{stdout}");
}
