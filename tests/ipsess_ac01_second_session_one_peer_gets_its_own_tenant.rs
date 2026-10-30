//! PRD-mcphost-implicit-signup-per-session-not-per-peer
//! AC1 (P0) -- Given the in-process test host and two initialized sessions
//! from one source IP and one User-Agent, When session A signs up with
//! handoff and redeems, and session B then calls `host.table.create` without
//! `tenant_key` inside the 10-minute window, Then B's call succeeds as an
//! implicit signup (`onboarding`, tenant `signup_source` "implicit") and the
//! tenants table holds two distinct tenants.

use crate::common;
use common::{McpClient, TestServer, extract_structured};
use serde_json::json;

const UA: &str = "ipsess-ac01/agent";

/// An initialized session: `initialize` first, so the host-minted
/// `Mcp-Session-Id` is echoed back on every later request.
async fn initialized_session(server: &TestServer) -> McpClient {
    let client = McpClient::new(&server.base_url).with_user_agent(UA).with_session_continuity();
    client.initialize().await;
    assert!(client.session_id().is_some(), "initialize must hand out a session id");
    client
}

#[tokio::test]
async fn ipsess_ac01() {
    let server = TestServer::start().await;

    let a = initialized_session(&server).await;
    let signup = extract_structured(
        &a.tools_call("signup", json!({"name": "ipsess-a", "handoff": true})).await.expect("A signup"),
    );
    let a_namespace = signup["tenant"].as_str().expect("tenant").to_string();
    let token = signup["handoff_token"].as_str().expect("handoff_token").to_string();
    a.tools_call("host.redeem", json!({"handoff_token": token})).await.expect("A redeem");
    assert_eq!(server.state.db.list_tenants().await.expect("tenants").len(), 1);

    // B: same IP, same User-Agent, its own initialized session, inside the window.
    let b = initialized_session(&server).await;
    assert_ne!(a.session_id(), b.session_id(), "two sessions");
    let created = extract_structured(
        &b.tools_call("host.table.create", json!({"name": "runs", "columns": {"id": "integer"}}))
            .await
            .expect("B's first keyless host.table.create must be an implicit signup, not a refusal"),
    );
    let b_namespace = created["onboarding"]["tenant"].as_str().expect("implicit signup onboarding.tenant").to_string();
    assert_ne!(b_namespace, a_namespace, "B is its own tenant: {created}");

    let tenants = server.state.db.list_tenants().await.expect("tenants");
    assert_eq!(tenants.len(), 2, "two distinct tenants");
    let b_row = tenants.iter().find(|t| t.namespace == b_namespace).expect("B's tenant row");
    assert_eq!(b_row.signup_source.as_deref(), Some("implicit"), "B's tenant is an implicit signup");
}
