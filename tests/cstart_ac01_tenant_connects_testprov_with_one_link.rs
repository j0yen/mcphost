//! PRD-mcphost-connect-start-and-callback
//! AC1 (P0) -- Given a fresh implicit-signup tenant with no end user and no
//! `vault_providers` row, When `host.connect.start {provider: "testprov"}` is
//! called, the link is followed through the test OAuth server and the
//! callback runs, Then `host.connect.list` shows `testprov {connected: true,
//! scopes: [<default>]}` and `vault_tokens` holds one row with
//! `end_user_subject = "tenant"` and a non-null `refresh_enc`.

use crate::common;
use common::testoauth::TestOAuth;
use common::{McpClient, TestServer, extract_structured};
use serde_json::json;

#[tokio::test]
async fn cstart_ac01() {
    let oauth = TestOAuth::start().await;
    let server = TestServer::start_with_connect_providers(oauth.connect_providers()).await;
    // A bare call from a fresh connection signs a tenant up implicitly.
    let agent = McpClient::new(&server.base_url).with_session_continuity();

    let started = agent
        .tools_call("host.connect.start", json!({"provider": "testprov"}))
        .await
        .expect("host.connect.start ok");
    let started = extract_structured(&started);
    // The implicit tenant's own URL: later calls reach the same tenant through it.
    let onboarding_url = started["onboarding"]["url"].as_str().expect("onboarding.url").to_string();
    let tenant_path = onboarding_url.strip_prefix(&server.state.public_url).expect("own url").to_string();
    let same_tenant = McpClient::new(&server.base_url).with_path(&tenant_path);
    let url = started["url"].as_str().expect("url").to_string();
    assert!(url.starts_with(&format!("{}/connections/go/", server.state.public_url)), "{url}");
    assert_eq!(started["expires_in_s"], 900);
    assert_eq!(started["provider"], "testprov");
    assert_eq!(started["scopes"], json!(["testprov.read"]));

    let tenant = server.state.db.find_tenant_by_id(1).await.unwrap().expect("the implicit tenant");
    assert!(
        server.state.db.list_vault_providers(tenant.id).await.unwrap().is_empty(),
        "the tenant registered no OAuth app of its own"
    );

    // The browser: go link -> test OAuth server -> callback.
    let callback_url = oauth.follow_to_callback(&url).await;
    assert!(callback_url.starts_with(&format!("{}/connections/callback/testprov?", server.state.public_url)), "{callback_url}");
    let seen = &oauth.authorize_requests()[0];
    assert_eq!(seen["scope"], "testprov.read");
    assert_eq!(seen["code_challenge_method"], "S256");
    assert_eq!(seen["redirect_uri"], format!("{}/connections/callback/testprov", server.state.public_url));

    let page = reqwest::get(&callback_url).await.expect("callback");
    assert_eq!(page.status(), reqwest::StatusCode::OK);
    let body = page.text().await.unwrap();
    assert!(body.contains("testprov connected to tenant"), "{body}");
    assert!(body.contains("you can close this tab"), "{body}");
    assert!(!body.contains("access-code"), "no token on the page: {body}");

    let listed = extract_structured(&same_tenant.tools_call("host.connect.list", json!({})).await.expect("list"));
    let row = listed["providers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["name"] == "testprov")
        .expect("testprov row")
        .clone();
    assert_eq!(row["connected"], true, "{row}");
    assert_eq!(row["scopes"], json!(["testprov.read"]), "{row}");
    assert!(!listed.to_string().contains("access-code"), "no token in list: {listed}");

    let stored = server
        .state
        .db
        .get_vault_token(tenant.id, "testprov".to_string(), "tenant".to_string())
        .await
        .unwrap()
        .expect("one tenant-level vault_tokens row");
    let refresh_enc = stored.refresh_enc.expect("refresh_enc is set");
    let refresh_nonce = stored.refresh_nonce.expect("refresh_nonce is set");
    assert_eq!(server.state.secrets.decrypt(&refresh_enc, &refresh_nonce).unwrap(), "refresh-code1");
    assert_eq!(server.state.secrets.decrypt(&stored.access_enc, &stored.access_nonce).unwrap(), "access-code1");
    assert!(stored.revoked_unix.is_none());
}
