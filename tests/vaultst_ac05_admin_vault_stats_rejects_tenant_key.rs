//! PRD-mcphost-upstream-token-vault-status
//! AC5 (P0) — Given a tenant key, When it calls `admin.vault.stats`, Then
//! the call is rejected the same way every other `admin.*` tool rejects a
//! tenant key.

use crate::common;
use common::{McpClient, TestServer, signup};
use serde_json::json;

#[tokio::test]
async fn tenant_key_calling_admin_vault_stats_is_forbidden() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "AC5 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let err = client
        .tools_call("admin.vault.stats", json!({}))
        .await
        .expect_err("a tenant key must never reach admin.vault.stats");
    assert_eq!(err.error_code.as_deref(), Some("forbidden"));
}
