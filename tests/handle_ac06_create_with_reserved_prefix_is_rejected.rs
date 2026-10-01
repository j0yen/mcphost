//! PRD-mcphost-result-handles
//! AC6 -- Given `host.table.create` with name `hdl_abc`, When called, Then
//! a validation error states the prefix is reserved.

use crate::common;
use common::{McpClient, TestServer, signup};
use serde_json::json;

#[tokio::test]
async fn create_with_hdl_prefix_is_rejected_as_reserved() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "ResultHandles AC6 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let err = client
        .tools_call("host.table.create", json!({"name": "hdl_abc", "columns": {"x": "text"}}))
        .await
        .expect_err("hdl_ prefix must be refused");
    assert_eq!(err.error_code.as_deref(), Some("args_invalid"), "err: {err:?}");
    assert!(err.message.contains("reserved"), "message must state the prefix is reserved: {err:?}");

    // requirement 6: host.table.drop refuses the same prefix too.
    let err_drop = client
        .tools_call("host.table.drop", json!({"name": "hdl_abc"}))
        .await
        .expect_err("hdl_ prefix must be refused on drop too");
    assert_eq!(err_drop.error_code.as_deref(), Some("args_invalid"), "err: {err_drop:?}");
    assert!(err_drop.message.contains("reserved"), "message must state the prefix is reserved: {err_drop:?}");
}
