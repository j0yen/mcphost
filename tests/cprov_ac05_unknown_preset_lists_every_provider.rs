//! PRD-mcphost-connection-provider-registry AC5 / R6: `host.vault.provider_set
//! {preset: "zzz"}` fails `invalid_params` and the message lists every name in
//! `Provider::ALL` (so the BYOO path shares the `providers.toml` table). A
//! known preset still fills the URLs from the same table, notion included --
//! a row that never had a hand-written preset.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use mcphost::connect::Provider;
use serde_json::json;

#[tokio::test]
async fn unknown_preset_is_invalid_params_naming_every_provider_in_all() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "AC5 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let err = client
        .tools_call(
            "host.vault.provider_set",
            json!({
                "name": "zz",
                "preset": "zzz",
                "client_id": "cid",
                "client_secret": "secret",
                "scopes": ["x"],
            }),
        )
        .await
        .expect_err("an unknown preset must be rejected");
    assert_eq!(err.error_code.as_deref(), Some("invalid_params"), "{err:?}");
    assert!(Provider::ALL.len() >= 4);
    for p in Provider::ALL {
        assert!(
            err.message.contains(p.name()),
            "message must list '{}': {}",
            p.name(),
            err.message
        );
    }
}

#[tokio::test]
async fn a_providers_toml_only_row_works_as_a_preset() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "AC5 Notion Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call(
            "host.vault.provider_set",
            json!({
                "name": "nt",
                "preset": "notion",
                "client_id": "cid",
                "client_secret": "secret",
                "scopes": [],
            }),
        )
        .await
        .expect("notion preset resolves through Provider::parse");
    let listed = client
        .tools_call("host.vault.providers", json!({}))
        .await
        .expect("host.vault.providers");
    let body = extract_structured(&listed);
    let row = body["providers"]
        .as_array()
        .and_then(|a| a.iter().find(|p| p["name"] == "nt"))
        .unwrap_or_else(|| panic!("nt listed: {body}"));
    assert_eq!(row["auth_url"], Provider::NOTION.spec().auth_url);
    assert_eq!(row["token_url"], Provider::NOTION.spec().token_url);
}
