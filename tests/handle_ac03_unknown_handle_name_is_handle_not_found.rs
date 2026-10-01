//! PRD-mcphost-result-handles
//! AC3 -- Given a handle name that was never created, When it is
//! referenced, Then `handle_not_found` names it and no rows return.

use crate::common;
use common::{McpClient, TestServer, signup};
use serde_json::json;

#[tokio::test]
async fn unknown_handle_reference_is_handle_not_found() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "ResultHandles AC3 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let err = client
        .tools_call(
            "host.table.query",
            json!({"sql": "SELECT * FROM hdl_000000000000"}),
        )
        .await
        .expect_err("a handle that was never created must be refused");
    assert_eq!(err.error_code.as_deref(), Some("handle_not_found"), "err: {err:?}");
    assert_eq!(err.data["handle"], "hdl_000000000000", "err: {err:?}");
}
