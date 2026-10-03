//! PRD-mcphost-result-handles
//! AC3 -- Given a handle name that was never created, When it is
//! referenced, Then `handle_not_found` names it and no rows return.

use crate::common;
use common::{McpClient, TestServer, signup};
use serde_json::json;

#[tokio::test]
async fn never_created_handle_reads_as_not_found() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Handle AC3 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let never_created = "hdl_000000000000";
    let err = client
        .tools_call("host.table.query", json!({"sql": format!("SELECT * FROM {never_created}")}))
        .await
        .expect_err("a handle name that was never created must be refused, not return rows");

    assert_eq!(err.error_code.as_deref(), Some("handle_not_found"), "error: {err:?}");
    assert!(
        err.message.contains(never_created),
        "error message must name the handle: {}",
        err.message
    );

    // A CTE/GROUP BY reference to the same nonexistent handle is refused
    // the same way -- requirement 3's check runs over every FROM/JOIN/CTE
    // reference, not just a bare `SELECT * FROM`.
    let err2 = client
        .tools_call(
            "host.table.query",
            json!({"sql": format!("SELECT category, SUM(amount) FROM {never_created} GROUP BY category")}),
        )
        .await
        .expect_err("a GROUP BY over a nonexistent handle must also be refused");
    assert_eq!(err2.error_code.as_deref(), Some("handle_not_found"), "error: {err2:?}");
}
