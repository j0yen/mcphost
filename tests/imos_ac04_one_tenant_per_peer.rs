//! PRD-mcphost-implicit-signup-mints-only-on-success
//! AC4 (P0) — Given a client that presents no server-issued session id and
//! whose first keyless call created tenant T, When the same IP and User-Agent
//! make a second keyless `host.*` call within 600 s, Then it is refused
//! `tenant_key_missing` with `data.reason` "peer_recent", NO `data.tenant`,
//! `data.session_header` "Mcp-Session-Id" and the nk401 R3 PeerRecent hint,
//! the tenant count stays 1, and `host.quickstart` and
//! explicit `signup` from that peer still succeed.

use crate::common;
use common::{McpClient, TestServer, extract_structured};
use serde_json::json;

#[tokio::test]
async fn a_header_ignoring_client_gets_one_tenant_per_peer_and_the_refusal_names_the_header() {
    let server = TestServer::start().await;
    let first = McpClient::new(&server.base_url).with_user_agent("imos-ac04/agent-sdk");
    let minted = first
        .tools_call("host.state.set", json!({"key": "k", "value": 1}))
        .await
        .expect("the first keyless call mints a tenant");
    let tenant = extract_structured(&minted)["onboarding"]["tenant"].as_str().expect("onboarding.tenant").to_string();

    // Same peer, a fresh stateless request (no session id echoed).
    let second = McpClient::new(&server.base_url).with_user_agent("imos-ac04/agent-sdk");
    let err = second
        .tools_call("host.state.get", json!({"key": "k"}))
        .await
        .expect_err("a second keyless call from the same peer is refused");
    assert_eq!(err.error_code.as_deref(), Some("tenant_key_missing"), "{err:?}");
    assert_eq!(err.data["reason"], json!("peer_recent"), "{err:?}");
    assert!(err.data.get("tenant").is_none(), "a peer refusal must not name tenant {tenant}: {err:?}");
    assert_eq!(
        err.data["hint"],
        json!("a tenant_key was redeemed from your address in the last 10 minutes; if that was you, pass it on every host.* call; if you are a different agent, call signup with handoff:true"),
        "{err:?}"
    );
    assert_eq!(err.data["session_header"], json!("Mcp-Session-Id"), "{err:?}");
    assert_eq!(server.state.db.list_tenants().await.expect("tenants").len(), 1, "the tenant count stays 1");

    second.tools_call("host.quickstart", json!({})).await.expect("host.quickstart is still served to the peer");
    let signed_up = second.tools_call("signup", json!({"name": "second-on-purpose"})).await.expect("explicit signup works");
    assert!(extract_structured(&signed_up)["key"].is_string(), "{signed_up}");
    assert_eq!(server.state.db.list_tenants().await.expect("tenants").len(), 2, "explicit signup still makes a second tenant");
}
