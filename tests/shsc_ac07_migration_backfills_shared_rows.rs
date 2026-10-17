//! PRD-mcphost-scoped-tool-share
//! AC7 (P0) -- Given the migration on a database with two shared tools and
//! one private, When it runs, Then the shared rows read
//! `["call","read_source"]` and the private row is NULL.

use serde_json::json;

use crate::common;
use common::{McpClient, TestServer, signup};

fn scopes_of(db_path: &std::path::Path, name: &str) -> Option<String> {
    let conn = rusqlite::Connection::open(db_path).expect("open raw db");
    conn.query_row("SELECT share_scopes_json FROM tools WHERE name = ?1", [name], |r| r.get(0))
        .expect("read share_scopes_json")
}

#[tokio::test]
async fn shsc_ac07_migration_backfills_two_shared_rows_and_leaves_private_null() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Owner A").await;
    let owner = McpClient::with_bearer(&server.base_url, &key);
    for name in ["pub_one", "pub_two", "priv_one"] {
        owner
            .tools_call(
                "host.tool_publish",
                json!({"name": name, "kind": "echo", "spec": {"schema": {"type": "object"}}}),
            )
            .await
            .expect("publish");
    }
    for name in ["pub_one", "pub_two"] {
        owner
            .tools_call("host.tool_share", json!({"name": name, "visibility": "public"}))
            .await
            .expect("share");
    }

    // Put the database back in its pre-0119 shape: the four columns are gone,
    // the two tools are still shared.
    let db_path = server.state.db.data_dir().join("mcphost.db");
    {
        let conn = rusqlite::Connection::open(&db_path).expect("open raw db");
        for col in ["share_scopes_json", "share_rate_per_hour", "unshared_unix", "forked_from"] {
            conn.execute_batch(&format!("ALTER TABLE tools DROP COLUMN {col}")).expect("drop column");
        }
    }

    server.state.db.migrate().await.expect("re-run migrations (0119)");

    assert_eq!(scopes_of(&db_path, "pub_one").as_deref(), Some(r#"["call","read_source"]"#));
    assert_eq!(scopes_of(&db_path, "pub_two").as_deref(), Some(r#"["call","read_source"]"#));
    assert_eq!(scopes_of(&db_path, "priv_one"), None, "a private row stays NULL");
}
