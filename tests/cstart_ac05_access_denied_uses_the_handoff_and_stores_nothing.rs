//! PRD-mcphost-connect-start-and-callback
//! AC5 (P0) -- Given the test server redirects with `?error=access_denied`,
//! When the callback runs, Then the page names `access_denied`, the handoff is
//! used, and `vault_tokens` has no row.

use crate::common;
use common::testoauth::TestOAuth;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn cstart_ac05() {
    let oauth = TestOAuth::start().await;
    oauth.deny_consent();
    let server = TestServer::start_with_connect_providers(oauth.connect_providers()).await;
    let (ns, key) = signup(&server.base_url, "CStart AC5").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    let tenant = server.state.db.find_tenant_by_namespace(ns).await.unwrap().expect("tenant");

    let url = extract_structured(&client.tools_call("host.connect.start", json!({"provider": "testprov"})).await.unwrap())
        ["url"]
        .as_str()
        .unwrap()
        .to_string();
    let callback_url = oauth.follow_to_callback(&url).await;
    assert!(callback_url.contains("error=access_denied"), "{callback_url}");

    let page = reqwest::get(&callback_url).await.unwrap();
    let body = page.text().await.unwrap();
    assert!(body.contains("access_denied"), "the page names the error: {body}");
    assert_eq!(oauth.token_calls(), 0, "a denial exchanges no code");

    assert!(
        server
            .state
            .db
            .get_vault_token(tenant.id, "testprov".to_string(), "tenant".to_string())
            .await
            .unwrap()
            .is_none(),
        "no vault_tokens row"
    );
    let handoff = server
        .state
        .db
        .find_vault_handoff_by_token_hash(mcphost::auth::hash_key(url.rsplit('/').next().unwrap()))
        .await
        .unwrap()
        .expect("the handoff row");
    assert!(handoff.oauth_state.starts_with("used:"), "the handoff is used: {}", handoff.oauth_state);

    // Used means used: replaying the denial is state_invalid, not a second page.
    let replay = reqwest::get(&callback_url).await.unwrap();
    assert_eq!(replay.status(), reqwest::StatusCode::BAD_REQUEST);
    assert!(replay.text().await.unwrap().contains("state_invalid"));

    // And the agent still sees the provider as not connected.
    let listed = extract_structured(&client.tools_call("host.connect.list", json!({})).await.unwrap());
    let row = listed["providers"].as_array().unwrap().iter().find(|p| p["name"] == "testprov").unwrap().clone();
    assert_eq!(row["connected"], false, "{row}");
}
