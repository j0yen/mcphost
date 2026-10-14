//! PRD-mcphost-oauth-first-entry-url
//! AC3 (P0) — Given a DCR client, When it authorizes with
//! `resource=<public_url>/login/mcp` and the consent form chooses "new
//! workspace", Then a tenant with `source: "oauth"` exists, the token's `aud`
//! is the entry resource, and `host.whoami` over `POST /login/mcp` with that
//! bearer returns that tenant with `onboarding.url`.

use crate::common;
use common::{McpClient, TestServer};
use mcphost::oauth::{OAUTH_ENTRY_PATH, entry_resource_uri};
use serde_json::json;

#[tokio::test]
async fn new_workspace_on_the_entry_resource_mints_an_entry_audience_token_that_works_on_the_entry_path() {
    let server = TestServer::start().await;
    let entry = format!("{}{OAUTH_ENTRY_PATH}", server.base_url);
    assert_eq!(entry, entry_resource_uri(&server.base_url));

    let before = server.state.db.list_tenants().await.expect("list tenants").len();
    let (_client_id, token) = common::oauth_consent_and_token(&server.base_url, &entry, &[("new_workspace", "1")]).await;
    let access_token = token["access_token"].as_str().unwrap_or_else(|| panic!("no access_token: {token}"));
    assert_eq!(common::jwt_aud(access_token), entry, "the token's aud is the entry resource");

    let tenants = server.state.db.list_tenants().await.expect("list tenants");
    assert_eq!(tenants.len(), before + 1, "exactly one tenant is created by the grant");

    let client = McpClient::with_bearer(&server.base_url, access_token).with_path(OAUTH_ENTRY_PATH);
    let who = common::extract_structured(&client.tools_call("host.whoami", json!({})).await.expect("whoami on entry"));
    assert_eq!(who["source"], json!("oauth"), "{who}");
    let tenant = who["tenant"].as_str().expect("tenant").to_string();
    assert!(tenants.iter().any(|t| t.namespace == tenant), "whoami names the tenant the grant created: {who}");
    assert!(
        who["onboarding"]["url"].as_str().is_some_and(|u| u.contains("/u/") && u.ends_with("/mcp")),
        "first call carries onboarding.url: {who}"
    );
}

#[tokio::test]
async fn tenant_key_consent_on_the_entry_resource_lands_in_the_existing_tenant() {
    let server = TestServer::start().await;
    let (ns, key) = common::signup(&server.base_url, "Returning Owner").await;
    let entry = entry_resource_uri(&server.base_url);

    let (_client_id, token) = common::oauth_consent_and_token(&server.base_url, &entry, &[("tenant_key", key.as_str())]).await;
    let access_token = token["access_token"].as_str().unwrap_or_else(|| panic!("no access_token: {token}"));
    assert_eq!(common::jwt_aud(access_token), entry);

    let client = McpClient::with_bearer(&server.base_url, access_token).with_path(OAUTH_ENTRY_PATH);
    let who = common::extract_structured(&client.tools_call("host.whoami", json!({})).await.expect("whoami on entry"));
    assert_eq!(who["tenant"], json!(ns), "a returning owner logs in to the existing tenant: {who}");
}

#[tokio::test]
async fn admin_oauth_stats_counts_registrations_and_calls_that_came_through_the_entry_url() {
    let server = TestServer::start_with(Some("admin-key-for-entry-stats".to_string())).await;
    let stats = |admin: McpClient| async move {
        common::extract_structured(&admin.tools_call("admin.oauth.stats", json!({})).await.expect("admin.oauth.stats"))
    };
    let before = stats(McpClient::with_bearer(&server.base_url, "admin-key-for-entry-stats")).await;
    assert_eq!(before["entry_url_registrations_7d"], json!(0), "{before}");
    assert_eq!(before["entry_url_calls_7d"], json!(0), "{before}");

    let entry = entry_resource_uri(&server.base_url);
    let (_cid, token) = common::oauth_consent_and_token(&server.base_url, &entry, &[("new_workspace", "1")]).await;
    let access_token = token["access_token"].as_str().expect("access_token").to_string();
    let client = McpClient::with_bearer(&server.base_url, &access_token).with_path(OAUTH_ENTRY_PATH);
    // `host.*` control-plane calls are not metered; publish a tool through the
    // entry URL and call it, the way a real connected client's first use would.
    let qualified = common::publish(
        &client,
        "echoer",
        "echo",
        json!({"schema": {"type": "object", "properties": {"msg": {"type": "string"}}}}),
    )
    .await;
    client.tools_call(&qualified, json!({"msg": "via-entry"})).await.expect("tool call on the entry path");

    let after = stats(McpClient::with_bearer(&server.base_url, "admin-key-for-entry-stats")).await;
    assert_eq!(after["entry_url_registrations_7d"], json!(1), "{after}");
    assert_eq!(after["entry_url_calls_7d"], json!(1), "{after}");
}
