//! PRD-mcphost-data-retention
//! AC9 (P1) — Given a prune that errors, When `healthz` is read, Then
//! `last_prune_ok` is false.
//!
//! The prune's batched deletes run on their own freshly-opened SQLite
//! connection (`retention::prune_sync`), separate from the app's own
//! already-open shared connection -- so chmod-ing the database file
//! read-only actually reproduces the fault for that fresh `open()` call
//! (unlike AC14's `is_writable` probe, which needs `PRAGMA query_only`
//! instead precisely because its connection is already open before any
//! chmod could apply -- see `Db::set_query_only`'s doc comment).

use std::os::unix::fs::PermissionsExt;

use crate::common;
use common::ADMIN_KEY;

async fn admin_healthz(base_url: &str) -> serde_json::Value {
    reqwest::Client::new()
        .get(format!("{base_url}/healthz"))
        .bearer_auth(ADMIN_KEY)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap()
}

#[tokio::test]
async fn prune_that_cannot_open_the_db_sets_last_prune_ok_false() {
    let server = common::TestServer::start().await;

    let health = admin_healthz(&server.base_url).await;
    assert_eq!(
        health["last_prune_ok"],
        serde_json::json!(true),
        "last_prune_ok must be true before any prune has run"
    );

    let db_path = server.state.db.data_dir().join("mcphost.db");
    let original_perms = std::fs::metadata(&db_path).expect("stat db file").permissions();
    std::fs::set_permissions(&db_path, std::fs::Permissions::from_mode(0o444))
        .expect("make db file read-only");

    let result = server.state.db.prune_once().await;

    // Restore writability before asserting, so a failed assertion doesn't
    // leave the temp dir behind in a state its own cleanup can't remove.
    std::fs::set_permissions(&db_path, original_perms).expect("restore db file permissions");

    result.expect_err("prune_once must fail when it cannot open the read-only db file");

    let health = admin_healthz(&server.base_url).await;
    assert_eq!(
        health["last_prune_ok"],
        serde_json::json!(false),
        "healthz must report last_prune_ok: false after a prune that errored"
    );
}
