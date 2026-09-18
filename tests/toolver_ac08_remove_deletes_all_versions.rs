//! AC8 — Given `host.tool_remove`, When called, Then all versions are
//! gone and `host.tool_history` returns not-found.

use crate::common;
use common::{McpClient, TestServer, publish, signup};
use serde_json::json;

#[tokio::test]
async fn remove_takes_every_stored_version_with_it() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Publisher").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    publish(&client, "greet", "echo", json!({"schema": {"type": "object"}})).await;
    publish(&client, "greet", "echo", json!({"schema": {"type": "object"}})).await;

    client
        .tools_call("host.tool_remove", json!({"name": "greet"}))
        .await
        .expect("remove greet");

    let err = client
        .tools_call("host.tool_history", json!({"name": "greet"}))
        .await
        .expect_err("history on a removed tool must be not-found");
    assert_eq!(err.error_code.as_deref(), Some("tool_not_found"));
}
