//! AC3 — Given an anonymous caller with an unrecognized `tenant_key`, When
//! it calls a tool, Then the response is `tenant_key_invalid` and `calls`
//! gains no row.

use crate::common;
use common::{McpClient, TestServer};
use serde_json::json;

#[tokio::test]
async fn unrecognized_tenant_key_refusal_writes_no_calls_row() {
    let server = TestServer::start().await;
    let client = McpClient::new(&server.base_url);
    let conn = rusqlite::Connection::open(server.data_dir.0.join("mcphost.db")).expect("open raw db");
    let count = |conn: &rusqlite::Connection| -> i64 {
        conn.query_row("SELECT COUNT(*) FROM calls", [], |r| r.get(0)).expect("count calls")
    };
    let before = count(&conn);

    let err = client
        .tools_call("host.tool_list", json!({"tenant_key": "t_notreal"}))
        .await
        .expect_err("an unrecognized tenant_key must be refused");
    assert_eq!(err.error_code.as_deref(), Some("tenant_key_invalid"));

    assert_eq!(count(&conn), before, "an anonymous refusal must not write a calls row");
}
