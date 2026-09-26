//! PRD-mcphost-upstream-token-vault-status
//! AC7 (P1) — Given `host.vault.provider_set {name: "gh", preset:
//! "github", client_id, client_secret, scopes: ["repo"]}` with no URLs,
//! When it runs, Then the stored row carries GitHub's authorize and
//! access-token URLs; Given the same call with an explicit `auth_url`,
//! Then the explicit value wins; Given `preset: "gitlab"`, Then
//! `invalid_params`.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

async fn provider_entry(client: &McpClient, name: &str) -> serde_json::Value {
    let result = client.tools_call("host.vault.providers", json!({})).await.expect("host.vault.providers ok");
    extract_structured(&result)["providers"]
        .as_array()
        .expect("providers array")
        .iter()
        .find(|p| p["name"] == json!(name))
        .unwrap_or_else(|| panic!("provider '{name}' not found"))
        .clone()
}

#[tokio::test]
async fn preset_fills_urls_explicit_overrides_and_unknown_preset_rejected() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "AC7 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call(
            "host.vault.provider_set",
            json!({
                "name": "gh",
                "preset": "github",
                "client_id": "gh-cid",
                "client_secret": "gh-secret",
                "scopes": ["repo"],
            }),
        )
        .await
        .unwrap_or_else(|e| panic!("preset-only provider_set must succeed: {} {}", e.code, e.message));

    let gh = provider_entry(&client, "gh").await;
    assert_eq!(gh["auth_url"], json!("https://github.com/login/oauth/authorize"), "{gh:?}");
    assert_eq!(gh["token_url"], json!("https://github.com/login/oauth/access_token"), "{gh:?}");

    // An explicit auth_url on top of the same preset wins over the preset's
    // own value; token_url (left unspecified this call) stays the preset's.
    client
        .tools_call(
            "host.vault.provider_set",
            json!({
                "name": "gh",
                "preset": "github",
                "auth_url": "https://github.example.invalid/custom-authorize",
                "client_id": "gh-cid",
                "client_secret": "gh-secret",
                "scopes": ["repo"],
            }),
        )
        .await
        .expect("explicit-auth_url-over-preset provider_set must succeed");
    let gh = provider_entry(&client, "gh").await;
    assert_eq!(gh["auth_url"], json!("https://github.example.invalid/custom-authorize"), "{gh:?}");
    assert_eq!(gh["token_url"], json!("https://github.com/login/oauth/access_token"), "{gh:?}");

    let err = client
        .tools_call(
            "host.vault.provider_set",
            json!({
                "name": "gl",
                "preset": "gitlab",
                "client_id": "gl-cid",
                "client_secret": "gl-secret",
                "scopes": ["read_api"],
            }),
        )
        .await
        .expect_err("an unknown preset must be rejected");
    assert_eq!(err.error_code.as_deref(), Some("invalid_params"));
}
