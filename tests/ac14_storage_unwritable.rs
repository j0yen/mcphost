//! AC14 — Given the database file is unwritable, When any `tools/call`
//! arrives, Then the server answers a JSON-RPC internal error naming
//! `storage`, `/healthz` reports `db_ok: false` (admin) and `{"ok": false}`
//! / 503 (anonymous, PRD-mcphost-healthz-minimal AC4), and the process
//! stays up.
//!
//! "Unwritable" is simulated via `PRAGMA query_only = ON` (see
//! `Db::set_query_only`'s doc comment): an already-open file descriptor
//! ignores a `chmod` on the underlying file, so that wouldn't reliably
//! reproduce the fault; `query_only` is SQLite's own first-class way to
//! make a connection reject writes with a real `SQLITE_READONLY` error,
//! which is what actually exercises the `Storage` mapping path.

use crate::common;
use common::{ADMIN_KEY, McpClient, TestServer, signup};
use reqwest::StatusCode;
use serde_json::json;

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

async fn anonymous_healthz(base_url: &str) -> (StatusCode, serde_json::Value) {
    let resp = reqwest::get(format!("{base_url}/healthz")).await.unwrap();
    let status = resp.status();
    (status, resp.json().await.unwrap())
}

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

    // healthz is healthy before the fault, anonymous and admin alike.
    let health = admin_healthz(&server.base_url).await;
    assert_eq!(health["db_ok"], json!(true));
    let (status, anon) = anonymous_healthz(&server.base_url).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(anon, json!({"ok": true}));

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

    let health = admin_healthz(&server.base_url).await;
    assert_eq!(
        health["db_ok"],
        json!(false),
        "healthz must report db_ok: false while unwritable"
    );
    let (status, anon) = anonymous_healthz(&server.base_url).await;
    assert_eq!(
        status,
        StatusCode::SERVICE_UNAVAILABLE,
        "anonymous healthz must 503 while the liveness check (db writability) fails"
    );
    assert_eq!(anon, json!({"ok": false}));

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
    let health = admin_healthz(&server.base_url).await;
    assert_eq!(
        health["db_ok"],
        json!(true),
        "healthz must recover once writable again"
    );
    let (status, anon) = anonymous_healthz(&server.base_url).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(anon, json!({"ok": true}));
}
