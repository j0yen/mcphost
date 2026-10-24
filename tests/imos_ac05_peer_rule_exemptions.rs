//! PRD-mcphost-implicit-signup-mints-only-on-success
//! AC5 (P0) — Given one peer, When (a) two clients that ran `initialize` and
//! echo `Mcp-Session-Id` each make a first keyless call, (b) a client with a
//! different User-Agent does, and (c) the R3 client's tenant T self-offboards
//! and the peer calls again, Then (a) creates two tenants, (b) creates its own
//! tenant, and (c) creates a new tenant and is never refused for T.

use crate::common;
use common::{McpClient, TestServer, extract_structured};
use serde_json::json;

const SHARED_UA: &str = "imos-ac05/shared";

async fn first_call_tenant(client: &McpClient) -> (String, String) {
    let out = extract_structured(
        &client
            .tools_call("host.state.set", json!({"key": "k", "value": 1}))
            .await
            .expect("a first keyless call mints a tenant"),
    );
    let onboarding = &out["onboarding"];
    (
        onboarding["tenant"].as_str().expect("onboarding.tenant").to_string(),
        onboarding["url"].as_str().expect("onboarding.url").to_string(),
    )
}

#[tokio::test]
async fn sessions_other_agents_and_offboarded_tenants_do_not_trip_the_peer_rule() {
    let server = TestServer::start_with_signup_rate_limit(1000).await;

    // (a) two clients that ran `initialize` and echo the id: one tenant each.
    let a1 = McpClient::new(&server.base_url).with_session_continuity().with_user_agent(SHARED_UA);
    let a2 = McpClient::new(&server.base_url).with_session_continuity().with_user_agent(SHARED_UA);
    a1.initialize().await;
    a2.initialize().await;
    let (t1, _) = first_call_tenant(&a1).await;
    let (t2, _) = first_call_tenant(&a2).await;
    assert_ne!(t1, t2, "(a) each session gets its own tenant");
    assert_eq!(server.state.db.list_tenants().await.expect("tenants").len(), 2);

    // (b) a different User-Agent is a different peer.
    let other = McpClient::new(&server.base_url).with_user_agent("imos-ac05/other");
    let (t3, _) = first_call_tenant(&other).await;
    assert!(t3 != t1 && t3 != t2, "(b) its own tenant");
    assert_eq!(server.state.db.list_tenants().await.expect("tenants").len(), 3);

    // (c) the R3 client (no session id) mints T, T self-offboards, the peer is free again.
    let r3 = McpClient::new(&server.base_url).with_user_agent("imos-ac05/r3");
    let (t, url) = first_call_tenant(&r3).await;
    let path = url.strip_prefix(&server.base_url).expect("url on this server").to_string();
    let on_url = McpClient::new(&server.base_url).with_path(&path);
    on_url.tools_call("host.self.offboard", json!({})).await.expect("T self-offboards on its url");
    let again = McpClient::new(&server.base_url).with_user_agent("imos-ac05/r3");
    let (t4, _) = first_call_tenant(&again).await;
    assert_ne!(t4, t, "(c) a new tenant, never a refusal naming T");
    assert_eq!(server.state.db.list_tenants().await.expect("tenants").len(), 5);
}
