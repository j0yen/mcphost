//! PRD-mcphost-connect-start-and-callback
//! AC6 (P0) -- Given a connected `testprov`, When `host.connect.revoke
//! {provider: "testprov"}` runs twice, Then both calls succeed, the row has
//! `revoked_reason: "tenant_revoke"`, and `host.connect.list` carries a
//! `reconnect_url`.

use crate::common;
use common::testoauth::TestOAuth;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::{Value, json};

async fn testprov_row(client: &McpClient) -> Value {
    let listed = extract_structured(&client.tools_call("host.connect.list", json!({})).await.unwrap());
    listed["providers"].as_array().unwrap().iter().find(|p| p["name"] == "testprov").unwrap().clone()
}

#[tokio::test]
async fn cstart_ac06() {
    let oauth = TestOAuth::start().await;
    let server = TestServer::start_with_connect_providers(oauth.connect_providers()).await;
    let (ns, key) = signup(&server.base_url, "CStart AC6").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    let tenant = server.state.db.find_tenant_by_namespace(ns).await.unwrap().expect("tenant");

    let url = extract_structured(&client.tools_call("host.connect.start", json!({"provider": "testprov"})).await.unwrap())
        ["url"]
        .as_str()
        .unwrap()
        .to_string();
    let callback_url = oauth.follow_to_callback(&url).await;
    assert_eq!(reqwest::get(&callback_url).await.unwrap().status(), reqwest::StatusCode::OK);
    let connected = testprov_row(&client).await;
    assert_eq!(connected["connected"], true, "{connected}");
    assert!(connected.get("reconnect_url").is_none(), "a live connection has no reconnect_url: {connected}");

    let first = extract_structured(&client.tools_call("host.connect.revoke", json!({"provider": "testprov"})).await.expect("first revoke"));
    assert_eq!(first["revoked_reason"], "tenant_revoke", "{first}");
    assert_eq!(first["provider_revoked"], true, "the registry's revoke_url was called: {first}");
    let revoked_at = first["revoked_at"].clone();
    let second = extract_structured(&client.tools_call("host.connect.revoke", json!({"provider": "testprov"})).await.expect("second revoke"));
    assert_eq!(second["revoked_reason"], "tenant_revoke", "{second}");
    assert_eq!(second["revoked_at"], revoked_at, "a repeat keeps the first revocation: {second}");
    assert_eq!(second["provider_revoked"], false, "a repeat has no live token to hand the provider: {second}");
    assert_eq!(oauth.revoked_tokens(), vec!["refresh-code1".to_string()], "the provider was asked once, with the refresh token");

    let row = server
        .state
        .db
        .get_vault_token(tenant.id, "testprov".to_string(), "tenant".to_string())
        .await
        .unwrap()
        .expect("the row stays, revoked");
    assert_eq!(row.revoked_reason.as_deref(), Some("tenant_revoke"));
    assert!(row.revoked_unix.is_some());

    let listed = testprov_row(&client).await;
    assert_eq!(listed["connected"], false, "{listed}");
    assert_eq!(listed["revoked_reason"], "tenant_revoke", "{listed}");
    let reconnect = listed["reconnect_url"].as_str().expect("a revoked connection carries reconnect_url");
    assert!(reconnect.starts_with(&format!("{}/connections/go/", server.state.public_url)), "{reconnect}");
    assert!(!listed.to_string().contains("code1"), "no token in list: {listed}");

    // Revoking a provider that was never connected also succeeds.
    let never = extract_structured(&client.tools_call("host.connect.revoke", json!({"provider": "github"})).await.expect("never connected"));
    assert_eq!(never["revoked"], false, "{never}");
}
