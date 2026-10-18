//! PRD-mcphost-self-offboard-releases-session
//! AC1 (P0) — Given a session that implicit-signed-up tenant A, When it
//! calls `host.self_offboard` key-less and then another `host.*` key-less on
//! the same session id, Then the second call succeeds as a new tenant B with
//! B != A, and the same sequence on a `host.redeem`-created session also
//! yields a fresh tenant.

use crate::common;
use common::{McpClient, TestServer, extract_structured};
use serde_json::json;

async fn whoami_tenant(client: &McpClient) -> String {
    extract_structured(&client.tools_call("host.whoami", json!({})).await.expect("host.whoami"))["tenant"]
        .as_str()
        .expect("tenant namespace")
        .to_string()
}

#[tokio::test]
async fn sorel_ac01_implicit_signup_session_gets_fresh_tenant_after_offboard() {
    let server = TestServer::start_with_signup_rate_limit(1000).await;
    let session = McpClient::new(&server.base_url).with_session_continuity();

    // The session's first key-less call implicit-signs-up tenant A, so the
    // offboard call itself is the call that creates and then closes A.
    let off = extract_structured(
        &session.tools_call("host.self.offboard", json!({})).await.expect("key-less self_offboard"),
    );
    let a = off["tenant"].as_str().expect("tenant A namespace").to_string();
    assert_eq!(off["disabled"], json!(true), "got {off}");

    let b = whoami_tenant(&session).await;
    assert_ne!(a, b, "the session must sign up a fresh tenant, not stay on A");
}

#[tokio::test]
async fn sorel_ac01_redeem_session_gets_fresh_tenant_after_offboard() {
    let server = TestServer::start_with_signup_rate_limit(1000).await;
    let raw = McpClient::new(&server.base_url)
        .tools_call("signup", json!({"name": "Sorel Redeem", "handoff": true}))
        .await
        .expect("signup(handoff)");
    let token = extract_structured(&raw)["handoff_token"].as_str().expect("token").to_string();

    let session = McpClient::new(&server.base_url).with_session_continuity();
    let redeemed = extract_structured(
        &session.tools_call("host.redeem", json!({"handoff_token": token})).await.expect("redeem"),
    );
    let key = redeemed["key"].as_str().expect("redeemed key").to_string();
    let a = extract_structured(
        &McpClient::with_bearer(&server.base_url, &key).tools_call("host.whoami", json!({})).await.expect("whoami"),
    )["tenant"]
        .as_str()
        .expect("redeemed tenant")
        .to_string();

    session.tools_call("host.self.offboard", json!({})).await.expect("key-less self_offboard");

    let b = whoami_tenant(&session).await;
    assert_ne!(a, b, "the redeem-created session must sign up a fresh tenant");
}
