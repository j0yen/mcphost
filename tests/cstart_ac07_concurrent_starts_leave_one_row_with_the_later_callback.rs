//! PRD-mcphost-connect-start-and-callback
//! AC7 (P0) -- Given two concurrent `host.connect.start` calls for the same
//! provider, When both callbacks complete, Then exactly one `vault_tokens` row
//! exists and it holds the tokens of the later callback.

use crate::common;
use common::testoauth::TestOAuth;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn cstart_ac07() {
    let oauth = TestOAuth::start().await;
    let server = TestServer::start_with_connect_providers(oauth.connect_providers()).await;
    let (ns, key) = signup(&server.base_url, "CStart AC7").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    let tenant = server.state.db.find_tenant_by_namespace(ns).await.unwrap().expect("tenant");

    let start = || async {
        extract_structured(&client.tools_call("host.connect.start", json!({"provider": "testprov"})).await.unwrap())["url"]
            .as_str()
            .unwrap()
            .to_string()
    };
    let (url_a, url_b) = tokio::join!(start(), start());
    assert_ne!(url_a, url_b, "each start mints its own link");

    // Both browsers reach the provider (codes are issued in this order) ...
    let callback_a = oauth.follow_to_callback(&url_a).await;
    let callback_b = oauth.follow_to_callback(&url_b).await;
    // ... and the callbacks complete in the opposite order: B first, A last.
    for callback in [&callback_b, &callback_a] {
        let page = reqwest::get(callback).await.unwrap();
        assert_eq!(page.status(), reqwest::StatusCode::OK);
    }

    let count = server.state.db.count_tenant_vault_tokens(tenant.id, "testprov".to_string()).await.unwrap();
    assert_eq!(count, 1, "one row per (tenant, provider, subject)");
    let row = server
        .state
        .db
        .get_vault_token(tenant.id, "testprov".to_string(), "tenant".to_string())
        .await
        .unwrap()
        .expect("row");
    // Link A's browser was issued code1, link B's code2; A's callback ran last.
    let access = server.state.secrets.decrypt(&row.access_enc, &row.access_nonce).unwrap();
    assert_eq!(access, "access-code1", "the later callback's tokens win");
    let refresh = server.state.secrets.decrypt(&row.refresh_enc.unwrap(), &row.refresh_nonce.unwrap()).unwrap();
    assert_eq!(refresh, "refresh-code1");
    assert!(row.revoked_unix.is_none());
}
