//! PRD-mcphost-tenant-key-missing-is-invalid-params
//! AC2 (P0) — Given the same call with `tenant_key: 12345` (non-string),
//! When the server responds, Then the numeric code is `-32602` and
//! `data.error_code` is `tenant_key_missing`.

use crate::common;
use common::{McpClient, TestServer};
use serde_json::json;

#[tokio::test]
async fn non_string_tenant_key_is_invalid_params() {
    let server = TestServer::start().await;
    let client = McpClient::new(&server.base_url);

    let err = client
        .tools_call(
            "host.tool_publish",
            json!({"name": "my_tool", "kind": "echo", "spec": {}, "tenant_key": 12345}),
        )
        .await
        .expect_err("a non-string tenant_key must be treated as absent");

    assert_eq!(err.code, -32602, "must be INVALID_PARAMS: {err:?}");
    assert_eq!(err.error_code.as_deref(), Some("tenant_key_missing"));
}
