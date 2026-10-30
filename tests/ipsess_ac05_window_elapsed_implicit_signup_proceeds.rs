//! PRD-mcphost-implicit-signup-per-session-not-per-peer
//! AC5 (P0) -- Given the window elapsed (mock clock past 600 s), When a
//! sessionless request from the same peer calls `host.table.create` without
//! `tenant_key`, Then implicit signup proceeds as in v0.118.8 (the TTL is
//! unchanged).

use crate::common;
use common::{McpClient, TestServer, extract_structured};
use mcphost::peer_recent::{PEER_TTL_SECS, peer_key};
use serde_json::json;

const UA: &str = "ipsess-ac05/agent";

fn create_args() -> serde_json::Value {
    json!({"name": "runs", "columns": {"id": "integer"}})
}

#[tokio::test]
async fn ipsess_ac05() {
    assert_eq!(PEER_TTL_SECS, 600, "the TTL is unchanged");
    let server = TestServer::start().await;

    let a = McpClient::new(&server.base_url).with_user_agent(UA).with_session_continuity();
    a.initialize().await;
    let signup = extract_structured(
        &a.tools_call("signup", json!({"name": "ipsess-a", "handoff": true})).await.expect("A signup"),
    );
    let a_namespace = signup["tenant"].as_str().expect("tenant").to_string();
    let token = signup["handoff_token"].as_str().expect("handoff_token").to_string();
    a.tools_call("host.redeem", json!({"handoff_token": token})).await.expect("A redeem");

    // Inside the window a sessionless request is still held by the gate.
    let sessionless = McpClient::new(&server.base_url).with_user_agent(UA);
    let held = sessionless
        .tools_call("host.table.create", create_args())
        .await
        .expect_err("inside the window the sessionless request is refused");
    assert_eq!(held.data["reason"], json!("peer_recent"), "{held:?}");

    // The mock clock: the same peer's redeem moved to 601 s ago.
    let peer = peer_key("127.0.0.1", Some(UA));
    let now = mcphost::state::now_unix();
    server.state.redeemed_peers.record_at(&peer, &a_namespace, a.session_id().as_deref(), now - PEER_TTL_SECS - 1);

    let created = extract_structured(
        &sessionless
            .tools_call("host.table.create", create_args())
            .await
            .expect("past the window the sessionless request is an implicit signup"),
    );
    let b_namespace = created["onboarding"]["tenant"].as_str().expect("implicit signup onboarding.tenant").to_string();
    assert_ne!(b_namespace, a_namespace, "{created}");

    let tenants = server.state.db.list_tenants().await.expect("tenants");
    assert_eq!(tenants.len(), 2, "two distinct tenants");
    let b_row = tenants.iter().find(|t| t.namespace == b_namespace).expect("the new tenant row");
    assert_eq!(b_row.signup_source.as_deref(), Some("implicit"));
}
