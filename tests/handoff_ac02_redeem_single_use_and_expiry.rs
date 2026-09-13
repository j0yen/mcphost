//! PRD-mcphost-handoff-token
//! AC2 (P0) — Given a valid token, When `host.redeem` is called, Then the
//! tenant key returns exactly once; a second redeem fails with the
//! structured already-redeemed error; an expired token fails with the
//! structured expired error.

use crate::common;
use common::{McpClient, TestServer, extract_structured};
use serde_json::json;

async fn handoff_signup(server: &TestServer) -> (String, i64) {
    let client = McpClient::new(&server.base_url);
    let raw = client
        .tools_call("signup", json!({"name": "Redeem Tenant", "handoff": true}))
        .await
        .expect("signup(handoff: true)");
    let result = extract_structured(&raw);
    let token = result["handoff_token"]
        .as_str()
        .expect("handoff_token field")
        .to_string();
    let expires_in = result["expires_in"].as_i64().expect("expires_in field");
    (token, expires_in)
}

#[tokio::test]
async fn redeem_returns_key_exactly_once() {
    let server = TestServer::start().await;
    let (token, _) = handoff_signup(&server).await;

    let client = McpClient::new(&server.base_url);
    let first = client
        .tools_call("host.redeem", json!({"handoff_token": token}))
        .await
        .expect("first redeem should succeed");
    let key = extract_structured(&first)["key"]
        .as_str()
        .expect("redeem returns a key")
        .to_string();
    assert!(!key.is_empty());

    let second = client
        .tools_call("host.redeem", json!({"handoff_token": token}))
        .await
        .expect_err("a second redemption of the same token must fail");
    assert_eq!(second.error_code.as_deref(), Some("handoff_token_redeemed"));
    assert!(
        !second.message.contains(&token),
        "redeem error must never echo the token: {}",
        second.message
    );

    // The key redeem returned must actually authenticate.
    let tenant_client = McpClient::with_bearer(&server.base_url, &key);
    tenant_client
        .tools_call("host.whoami", json!({}))
        .await
        .expect("the redeemed key must authenticate");
}

#[tokio::test]
async fn redeem_unrecognized_token_is_refused_without_echoing_it() {
    let server = TestServer::start().await;
    let client = McpClient::new(&server.base_url);
    let err = client
        .tools_call("host.redeem", json!({"handoff_token": "ho_not-a-real-token"}))
        .await
        .expect_err("an unrecognized token must be refused");
    assert_eq!(err.error_code.as_deref(), Some("handoff_token_invalid"));
    assert!(!err.message.contains("ho_not-a-real-token"));
}

#[tokio::test]
async fn redeem_expired_token_fails_distinctly_from_already_redeemed() {
    let server = TestServer::start().await;
    let (token, _) = handoff_signup(&server).await;

    // Force the token past its expiry without a real sleep -- same
    // "flip an internal knob" shape as AC14's `Db::set_query_only`.
    let token_hash = mcphost::auth::hash_key(&token);
    server
        .state
        .db
        .expire_handoff_token_for_test(token_hash)
        .await
        .expect("force-expire the token");

    let client = McpClient::new(&server.base_url);
    let err = client
        .tools_call("host.redeem", json!({"handoff_token": token}))
        .await
        .expect_err("an expired token must be refused");
    assert_eq!(err.error_code.as_deref(), Some("handoff_token_expired"));
}
