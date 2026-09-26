//! PRD-mcphost-upstream-token-vault
//! AC9 (P0) — Given `host.vault.providers`, When called, Then
//! `client_secret` is absent from the output and present, encrypted, in the
//! table.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

const CLIENT_SECRET: &str = "super-secret-oauth-client-value";

#[tokio::test]
async fn providers_list_never_returns_the_client_secret() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "AC9 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call(
            "host.vault.provider_set",
            json!({
                "name": "slack",
                "auth_url": "https://slack.example.invalid/authorize",
                "token_url": "https://slack.example.invalid/token",
                "client_id": "cid-123",
                "client_secret": CLIENT_SECRET,
                "scopes": ["read", "write"],
            }),
        )
        .await
        .expect("provider_set ok");

    let result = client
        .tools_call("host.vault.providers", json!({}))
        .await
        .expect("providers ok");
    let structured = extract_structured(&result);
    let providers = structured["providers"].as_array().expect("providers array");
    assert_eq!(providers.len(), 1);
    let provider = &providers[0];
    assert_eq!(provider["name"], "slack");
    assert!(provider.get("client_secret").is_none(), "client_secret must be absent: {provider}");
    let dump = structured.to_string();
    assert!(!dump.contains(CLIENT_SECRET), "the raw client_secret must never appear in the output: {dump}");

    let tenant = server
        .state
        .db
        .find_tenant_by_namespace(ns)
        .await
        .unwrap()
        .expect("tenant");
    let row = server
        .state
        .db
        .get_vault_provider(tenant.id, "slack".to_string())
        .await
        .expect("db read ok")
        .expect("provider row exists");
    assert_ne!(row.client_secret_enc, CLIENT_SECRET.as_bytes(), "must be encrypted at rest");
    let decrypted = server
        .state
        .secrets
        .decrypt(&row.client_secret_enc, &row.client_secret_nonce)
        .expect("decrypt");
    assert_eq!(decrypted, CLIENT_SECRET, "the table must hold the real secret, just encrypted");
}
