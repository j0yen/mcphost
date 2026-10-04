//! PRD-mcphost-tenant-key-missing-is-invalid-params
//! AC1 (P0) — Given an argument-only `tools/call` with no `Authorization`
//! header and no `tenant_key`, When the server responds, Then the
//! JSON-RPC error `code` is `-32602` (`INVALID_PARAMS`) and not `-32600`,
//! `data.error_code` is `tenant_key_missing`, and `data.docs` is
//! `host.quickstart`.
//!
//! PRD-mcphost-implicit-signup: a bare `host.*` call on `/mcp` with no
//! `tenant_key` at all now implicitly signs up instead of refusing (see
//! tests/implsign_ac01_*.rs), so `admin.tenants` -- which never
//! implicit-signs-up (Non-goal) -- is this test's conduit instead of
//! `host.tool_publish`.

use crate::common;
use common::{McpClient, TestServer};
use serde_json::json;

#[tokio::test]
async fn missing_tenant_key_is_invalid_params_not_invalid_request() {
    let server = TestServer::start().await;
    // No Authorization header, and no tenant_key argument either.
    let client = McpClient::new(&server.base_url);

    let err = client
        .tools_call("admin.tenants", json!({}))
        .await
        .expect_err("a call with no tenant_key at all must be refused");

    assert_eq!(err.code, -32602, "must be INVALID_PARAMS, not INVALID_REQUEST (-32600): {err:?}");
    assert_eq!(err.error_code.as_deref(), Some("tenant_key_missing"));
    assert_eq!(err.data.get("docs").and_then(|v| v.as_str()), Some("host.quickstart"));
}
