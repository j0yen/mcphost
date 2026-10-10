//! AC6 — Given the database is unwritable, When a call is refused, Then the
//! refusal response is still returned (the row write failure is logged, not
//! surfaced), unlike the success path's AC14 contract.

use crate::common;
use common::{McpClient, TestServer, signup};
use serde_json::json;

#[tokio::test]
async fn refusal_response_survives_an_unwritable_database() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "Refused Unwritable Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    client
        .tools_call(
            "host.tool_publish",
            json!({
                "name": "intool",
                "kind": "echo",
                "spec": {"schema": {
                    "type": "object",
                    "properties": {"n": {"type": "integer"}},
                    "required": ["n"]
                }}
            }),
        )
        .await
        .expect("publish while writable");

    server.state.db.set_query_only(true).await.expect("simulate unwritable db");

    let err = client
        .tools_call(&format!("{ns}.intool"), json!({"n": 4.78}))
        .await
        .expect_err("still refused");
    assert_eq!(
        err.error_code.as_deref(),
        Some("args_invalid"),
        "the refusal must be returned, not a storage error: {err:?}"
    );

    server.state.db.set_query_only(false).await.expect("restore writes");
    let conn = rusqlite::Connection::open(server.data_dir.0.join("mcphost.db")).expect("open raw db");
    let n: i64 = conn
        .query_row("SELECT COUNT(*) FROM calls WHERE error_code IS NOT NULL", [], |r| r.get(0))
        .expect("count");
    assert_eq!(n, 0, "the failed row write must leave no row behind");
}
