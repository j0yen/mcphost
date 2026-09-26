//! PRD-mcphost-upstream-token-vault-status
//! AC6 (P0) — Given provider `slack` with three connected users, When the
//! tenant calls `host.vault.provider_remove {name: "slack"}`, Then
//! `host.vault.providers` no longer lists it, all three `vault_tokens`
//! rows carry `revoked_reason = "provider_removed"`, and each user's
//! `host.vault.status` shows `connected: false` with that reason; When the
//! name is unknown, Then `not_found`.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

#[tokio::test]
async fn provider_remove_revokes_every_users_token_and_delists_the_provider() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "AC6 Tenant").await;
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

    let tenant = server.state.db.find_tenant_by_namespace(ns).await.unwrap().expect("tenant");
    for user in ["u1", "u2", "u3"] {
        let (access_enc, access_nonce) = server.state.secrets.encrypt(&format!("{user}-token")).expect("encrypt");
        server
            .state
            .db
            .upsert_vault_token(
                tenant.id,
                "slack".to_string(),
                user.to_string(),
                None,
                access_enc,
                access_nonce,
                None,
                None,
                now_unix() + 3600,
                "read".to_string(),
            )
            .await
            .expect("seed vault token");
    }

    let result = client
        .tools_call("host.vault.provider_remove", json!({"name": "slack"}))
        .await
        .unwrap_or_else(|e| panic!("provider_remove must succeed: {} {}", e.code, e.message));
    assert_eq!(extract_structured(&result)["removed"], json!(true));

    let providers_result = client
        .tools_call("host.vault.providers", json!({}))
        .await
        .expect("host.vault.providers ok");
    let providers = extract_structured(&providers_result)["providers"]
        .as_array()
        .expect("providers array")
        .clone();
    assert!(
        !providers.iter().any(|p| p["name"] == json!("slack")),
        "slack must no longer be a registered provider: {providers:?}"
    );

    for user in ["u1", "u2", "u3"] {
        let row = server
            .state
            .db
            .get_vault_token(tenant.id, "slack".to_string(), user.to_string())
            .await
            .expect("db read ok")
            .unwrap_or_else(|| panic!("{user}'s vault_tokens row must still exist"));
        assert!(row.revoked_unix.is_some(), "{user}'s token must be revoked");
        assert_eq!(row.revoked_reason.as_deref(), Some("provider_removed"), "{user:?}");

        let status_result = client
            .tools_call("host.vault.status", json!({"end_user": user}))
            .await
            .unwrap_or_else(|e| panic!("host.vault.status for {user} must succeed: {} {}", e.code, e.message));
        let structured = extract_structured(&status_result);
        let providers = structured["providers"].as_array().expect("providers array");
        let slack = providers
            .iter()
            .find(|p| p["name"] == json!("slack"))
            .unwrap_or_else(|| panic!("{user}'s status must still name the removed slack provider: {providers:?}"));
        assert_eq!(slack["connected"], json!(false), "{slack:?}");
        assert_eq!(slack["revoked_reason"], json!("provider_removed"), "{slack:?}");
    }

    let err = client
        .tools_call("host.vault.provider_remove", json!({"name": "unknown-provider"}))
        .await
        .expect_err("an unknown provider name must be rejected");
    assert_eq!(err.error_code.as_deref(), Some("not_found"));
}
