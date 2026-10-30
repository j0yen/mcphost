//! PRD-mcphost-webhook-inbox-one-call
//! AC4 -- Given a peer that completed `host.redeem` 5 s earlier, When it
//! sends `host.tool_publish` without `tenant_key` on `/mcp`, Then the error
//! is `tenant_key_missing` with `data.tenant` equal to that namespace, the
//! `tenants` row count is unchanged, and a peer with no redeem in 10
//! minutes still gets implicit signup.

use crate::common;

use common::{McpClient, TestServer, extract_structured};
use mcphost::peer_recent::{PEER_TTL_SECS, peer_key};
use serde_json::json;

fn publish_args() -> serde_json::Value {
    json!({"name": "hello", "kind": "echo", "spec": {"schema": {"type": "object"}}})
}

#[tokio::test]
async fn keyless_publish_after_redeem_is_refused_naming_the_tenant_and_makes_none() {
    let server = TestServer::start().await;
    let peer = McpClient::new(&server.base_url);

    let signup = extract_structured(
        &peer
            .tools_call("signup", json!({"name": "Redeemer", "handoff": true}))
            .await
            .expect("signup handoff"),
    );
    let namespace = signup["tenant"].as_str().expect("tenant").to_string();
    let token = signup["handoff_token"].as_str().expect("handoff_token").to_string();
    let redeemed = extract_structured(
        &peer.tools_call("host.redeem", json!({"handoff_token": token})).await.expect("redeem"),
    );
    let key = redeemed["key"].as_str().expect("redeemed key").to_string();
    let tenants_before = server.state.db.list_tenants().await.expect("list tenants").len();
    assert_eq!(tenants_before, 1);

    // The same peer, a fresh stateless request: no tenant_key, no session.
    let refused = McpClient::new(&server.base_url)
        .tools_call("host.tool_publish", publish_args())
        .await
        .expect_err("a key-less publish right after redeem must be refused");
    assert_eq!(refused.error_code.as_deref(), Some("tenant_key_missing"), "{refused:?}");
    // PRD-mcphost-named-key-refusal-not-http-401: the peer-recent refusal
    // names no tenant on the wire (a different agent at the same address
    // must not learn it).
    assert_eq!(refused.data["reason"], json!("peer_recent"), "{refused:?}");
    assert!(refused.data.get("tenant").is_none(), "{refused:?}");
    assert!(!format!("{refused:?}").contains(&namespace), "{refused:?}");
    assert!(!format!("{refused:?}").contains(&key), "never the key: {refused:?}");
    assert_eq!(refused.data["docs"], json!("host.quickstart"), "{refused:?}");
    assert_eq!(
        server.state.db.list_tenants().await.expect("list tenants").len(),
        tenants_before,
        "no second tenant"
    );

    // The pre-auth pages stay readable for that peer.
    McpClient::new(&server.base_url)
        .tools_call("host.quickstart", json!({}))
        .await
        .expect("quickstart stays anonymous");
}

#[tokio::test]
async fn a_peer_with_no_redeem_in_ten_minutes_still_gets_implicit_signup() {
    let server = TestServer::start().await;

    // A peer that redeemed 10 minutes and a second ago: forgotten.
    let peer = peer_key("127.0.0.1", None);
    server
        .state
        .redeemed_peers
        .record_at(&peer, "t_stale", None, mcphost::state::now_unix() - PEER_TTL_SECS - 1);

    let published = extract_structured(
        &McpClient::new(&server.base_url)
            .tools_call("host.tool_publish", publish_args())
            .await
            .expect("an expired redeem must not block implicit signup"),
    );
    assert!(published["onboarding"]["tenant"].is_string(), "{published}");
    assert_eq!(server.state.db.list_tenants().await.expect("list tenants").len(), 1);
}

#[tokio::test]
async fn a_peer_that_never_redeemed_gets_implicit_signup() {
    let server = TestServer::start().await;
    let published = extract_structured(
        &McpClient::new(&server.base_url)
            .tools_call("host.tool_publish", publish_args())
            .await
            .expect("fresh peer"),
    );
    assert!(published["onboarding"]["tenant"].is_string(), "{published}");
}
