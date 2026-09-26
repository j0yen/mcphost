//! PRD-mcphost-upstream-token-vault
//! AC11 (P1) — Given plan `vault_providers_max=2`, When a third
//! `provider_set` runs, Then it is rejected with `quota_vault_providers`.
//!
//! The `free` plan's own default is `vault_providers_max: 2`
//! (requirement 2), so a fresh signup already carries the plan this AC
//! names -- no plan override needed.

use crate::common;
use common::{McpClient, TestServer, signup};
use serde_json::json;

fn provider_args(name: &str) -> serde_json::Value {
    json!({
        "name": name,
        "auth_url": "https://example.invalid/authorize",
        "token_url": "https://example.invalid/token",
        "client_id": "cid",
        "client_secret": "shh",
        "scopes": ["read"],
    })
}

#[tokio::test]
async fn a_third_provider_is_rejected_once_the_plan_quota_is_reached() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "AC11 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call("host.vault.provider_set", provider_args("p1"))
        .await
        .expect("first provider must succeed");
    client
        .tools_call("host.vault.provider_set", provider_args("p2"))
        .await
        .expect("second provider must succeed");

    let err = client
        .tools_call("host.vault.provider_set", provider_args("p3"))
        .await
        .expect_err("a third provider must be rejected");
    assert_eq!(err.error_code.as_deref(), Some("quota_vault_providers"));

    // Re-setting an already-registered provider never counts against the
    // quota (same convention `control::secret_set` uses for `secrets_max`).
    client
        .tools_call("host.vault.provider_set", provider_args("p1"))
        .await
        .expect("re-setting an existing provider must still succeed at the quota");
}
