//! PRD-mcphost-upstream-token-vault-status
//! AC4 (P0) — Given tenant t1 with two slack users and one refresh failure
//! in the last 24 h, and tenant t2 with one slack and one github user,
//! When the admin key calls `admin.vault.stats`, Then `tenants` lists t1
//! `slack {tokens: 2, revoked: 1, refresh_failures_24h: 1}` and t2 `slack
//! {tokens: 1}` and `github {tokens: 1}`, `totals.tokens == 4`, and no
//! token, secret, or `client_secret` substring appears.

use crate::common;
use common::{ADMIN_KEY, McpClient, TestServer, extract_structured, signup};
use serde_json::json;

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

async fn seed_token(server: &common::TestServer, tenant_id: i64, provider: &str, subject: &str, plaintext_token: &str) {
    let (access_enc, access_nonce) = server.state.secrets.encrypt(plaintext_token).expect("encrypt");
    server
        .state
        .db
        .upsert_vault_token(
            tenant_id,
            provider.to_string(),
            subject.to_string(),
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

#[tokio::test]
async fn admin_vault_stats_reports_per_tenant_per_provider_counts() {
    let server = TestServer::start().await;

    let (ns1, key1) = signup(&server.base_url, "AC4 Tenant One").await;
    let client1 = McpClient::with_bearer(&server.base_url, &key1);
    client1
        .tools_call(
            "host.vault.provider_set",
            json!({
                "name": "slack",
                "auth_url": "https://slack.example.invalid/authorize",
                "token_url": "https://slack.example.invalid/token",
                "client_id": "t1-cid",
                "client_secret": "t1-shh-secret",
                "scopes": ["read"],
            }),
        )
        .await
        .expect("t1 provider_set slack ok");
    let tenant1 = server.state.db.find_tenant_by_namespace(ns1.clone()).await.unwrap().expect("tenant1");
    seed_token(&server, tenant1.id, "slack", "a1", "t1-a1-token").await;
    seed_token(&server, tenant1.id, "slack", "a2", "t1-a2-token").await;
    server
        .state
        .db
        .revoke_vault_token(tenant1.id, "slack".to_string(), "a2".to_string(), Some("refresh_http_401".to_string()))
        .await
        .expect("revoke t1 a2");

    let (ns2, key2) = signup(&server.base_url, "AC4 Tenant Two").await;
    let client2 = McpClient::with_bearer(&server.base_url, &key2);
    client2
        .tools_call(
            "host.vault.provider_set",
            json!({
                "name": "slack",
                "auth_url": "https://slack.example.invalid/authorize",
                "token_url": "https://slack.example.invalid/token",
                "client_id": "t2-cid",
                "client_secret": "t2-shh-secret",
                "scopes": ["read"],
            }),
        )
        .await
        .expect("t2 provider_set slack ok");
    client2
        .tools_call(
            "host.vault.provider_set",
            json!({
                "name": "github",
                "auth_url": "https://github.example.invalid/authorize",
                "token_url": "https://github.example.invalid/token",
                "client_id": "t2-gh-cid",
                "client_secret": "t2-gh-shh-secret",
                "scopes": ["repo"],
            }),
        )
        .await
        .expect("t2 provider_set github ok");
    let tenant2 = server.state.db.find_tenant_by_namespace(ns2.clone()).await.unwrap().expect("tenant2");
    seed_token(&server, tenant2.id, "slack", "b1", "t2-b1-token").await;
    seed_token(&server, tenant2.id, "github", "c1", "t2-c1-token").await;

    let admin = McpClient::with_bearer(&server.base_url, ADMIN_KEY);
    let result = admin
        .tools_call("admin.vault.stats", json!({}))
        .await
        .unwrap_or_else(|e| panic!("admin.vault.stats must succeed: {} {}", e.code, e.message));
    let structured = extract_structured(&result);
    let tenants = structured["tenants"].as_array().expect("tenants array");

    let t1 = tenants
        .iter()
        .find(|t| t["tenant_id"] == json!(ns1))
        .unwrap_or_else(|| panic!("t1 missing from tenants: {tenants:?}"));
    let t1_providers = t1["providers"].as_array().expect("t1 providers array");
    let t1_slack = t1_providers.iter().find(|p| p["name"] == json!("slack")).expect("t1 slack entry");
    assert_eq!(t1_slack["tokens"], json!(2), "{t1_slack:?}");
    assert_eq!(t1_slack["revoked"], json!(1), "{t1_slack:?}");
    assert_eq!(t1_slack["refresh_failures_24h"], json!(1), "{t1_slack:?}");

    let t2 = tenants
        .iter()
        .find(|t| t["tenant_id"] == json!(ns2))
        .unwrap_or_else(|| panic!("t2 missing from tenants: {tenants:?}"));
    let t2_providers = t2["providers"].as_array().expect("t2 providers array");
    let t2_slack = t2_providers.iter().find(|p| p["name"] == json!("slack")).expect("t2 slack entry");
    assert_eq!(t2_slack["tokens"], json!(1), "{t2_slack:?}");
    let t2_github = t2_providers.iter().find(|p| p["name"] == json!("github")).expect("t2 github entry");
    assert_eq!(t2_github["tokens"], json!(1), "{t2_github:?}");

    assert_eq!(structured["totals"]["tokens"], json!(4), "{structured:?}");

    let raw = structured.to_string();
    for needle in [
        "t1-a1-token",
        "t1-a2-token",
        "t2-b1-token",
        "t2-c1-token",
        "t1-shh-secret",
        "t2-shh-secret",
        "t2-gh-shh-secret",
        "client_secret",
    ] {
        assert!(!raw.contains(needle), "response must never carry '{needle}': {raw}");
    }
}
