//! AC14 — Given the database file is unwritable, When any `tools/call`
//! arrives, Then the server answers a JSON-RPC internal error naming
//! `storage`, `/healthz` reports `db_ok: false`, and the process stays up.
//!
//! "Unwritable" is simulated via `PRAGMA query_only = ON` (see
//! `Db::set_query_only`'s doc comment): an already-open file descriptor
//! ignores a `chmod` on the underlying file, so that wouldn't reliably
//! reproduce the fault; `query_only` is SQLite's own first-class way to
//! make a connection reject writes with a real `SQLITE_READONLY` error,
//! which is what actually exercises the `Storage` mapping path.

mod common;
use common::{McpClient, TestServer, signup};
use serde_json::json;

#[tokio::test]
async fn unwritable_database_degrades_gracefully_and_recovers() {
    let server = TestServer::start().await;
    let (tenant_ns, key) = signup(&server.base_url, "Storage Test Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "hello", "kind": "echo", "spec": {"schema": {"type": "object"}}}),
        )
        .await
        .expect("publish while writable");
    let qualified = format!("{tenant_ns}.hello");

    // healthz is healthy before the fault.
    let health: serde_json::Value = reqwest::get(format!("{}/healthz", server.base_url))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(health["db_ok"], json!(true));

    server
        .state
        .db
        .set_query_only(true)
        .await
        .expect("simulate unwritable db");

    let err = client
        .tools_call(&qualified, json!({}))
        .await
        .expect_err("tools/call must fail while the db is unwritable");
    assert_eq!(err.error_code.as_deref(), Some("storage"));

    let health: serde_json::Value = reqwest::get(format!("{}/healthz", server.base_url))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        health["db_ok"],
        json!(false),
        "healthz must report db_ok: false while unwritable"
    );

    // The process stays up: a read-only-safe endpoint keeps answering.
    let tools = client
        .tools_list()
        .await
        .expect("tools/list must still work (it's a read)");
    assert!(tools["tools"].is_array());

    server
        .state
        .db
        .set_query_only(false)
        .await
        .expect("restore writability");
    let health: serde_json::Value = reqwest::get(format!("{}/healthz", server.base_url))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        health["db_ok"],
        json!(true),
        "healthz must recover once writable again"
    );
}
