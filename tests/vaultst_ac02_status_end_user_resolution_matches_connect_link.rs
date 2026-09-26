//! PRD-mcphost-upstream-token-vault-status
//! AC2 (P0) — Given the same rows, When the tenant's agent (no end-user
//! identity on the call) calls `host.vault.status {end_user: "<u1
//! subject>"}`, Then the same view returns; When `end_user` is omitted or
//! `"self"` without an identity, Then `invalid_params` and
//! `upstream_not_connected` respectively, matching `connect_link`'s
//! behaviour.

use crate::common;
use common::{McpClient, TestServer, signup};
use serde_json::json;

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

#[tokio::test]
async fn tenant_agent_reads_literal_subject_same_view_as_self() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "AC2 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call(
            "host.vault.provider_set",
            json!({
                "name": "slack",
                "auth_url": "https://slack.example.invalid/authorize",
                "token_url": "https://slack.example.invalid/token",
                "client_id": "cid-123",
                "client_secret": "shh-secret",
                "scopes": ["read"],
            }),
        )
        .await
        .expect("provider_set ok");

    let tenant = server
        .state
        .db
        .find_tenant_by_namespace(ns)
        .await
        .unwrap()
        .expect("tenant");
    let (access_enc, access_nonce) = server.state.secrets.encrypt("u1-token").expect("encrypt");
    server
        .state
        .db
        .upsert_vault_token(
            tenant.id,
            "slack".to_string(),
            "u1".to_string(),
            None,
            access_enc,
            access_nonce,
            None,
            None,
            now_unix() + 3600,
            "read".to_string(),
        )
        .await
        .expect("seed vault token for u1");

    // The tenant's own agent, no end-user identity on the call, reads u1's
    // status via a literal subject string.
    let result = client
        .tools_call("host.vault.status", json!({"end_user": "u1"}))
        .await
        .unwrap_or_else(|e| panic!("host.vault.status with literal subject must succeed: {} {}", e.code, e.message));
    let structured = common::extract_structured(&result);
    let providers = structured["providers"].as_array().expect("providers array");
    let slack = providers.iter().find(|p| p["name"] == json!("slack")).expect("slack entry");
    assert_eq!(slack["connected"], json!(true), "{slack:?}");

    // `end_user` omitted -> invalid_params.
    let err = client
        .tools_call("host.vault.status", json!({}))
        .await
        .expect_err("omitted end_user must be rejected");
    assert_eq!(err.error_code.as_deref(), Some("invalid_params"), "{err:?}");

    // `end_user: "self"` with no verified identity on the call ->
    // upstream_not_connected, matching connect_link's own not-connected
    // shape for an unidentifiable caller.
    let err = client
        .tools_call("host.vault.status", json!({"end_user": "self"}))
        .await
        .expect_err("\"self\" with no identity must be rejected");
    assert_eq!(err.error_code.as_deref(), Some("upstream_not_connected"), "{err:?}");
}
