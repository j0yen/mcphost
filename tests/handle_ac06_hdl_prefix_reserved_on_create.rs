//! PRD-mcphost-result-handles
//! AC6 -- Given `host.table.create` with name `hdl_abc`, When called, Then
//! a validation error states the prefix is reserved.

use crate::common;
use common::{McpClient, TestServer, signup};
use serde_json::json;

#[tokio::test]
async fn create_with_hdl_prefix_is_refused_as_reserved() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Handle AC6 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let err = client
        .tools_call("host.table.create", json!({"name": "hdl_abc", "columns": {"x": "text"}}))
        .await
        .expect_err("a table name starting with hdl_ must be refused");
    assert!(
        err.message.contains("reserved"),
        "error must state the prefix is reserved: {}",
        err.message
    );
    assert!(err.message.contains("hdl_"), "error message: {}", err.message);

    // requirement 6: host.table.drop refuses the same prefix too (a
    // handle is dropped through host.table.handle_drop, never this tool).
    let err2 = client
        .tools_call("host.table.drop", json!({"name": "hdl_abc"}))
        .await
        .expect_err("host.table.drop must also refuse the hdl_ prefix");
    assert!(
        err2.message.contains("reserved"),
        "error must state the prefix is reserved: {}",
        err2.message
    );
}
