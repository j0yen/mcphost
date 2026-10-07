//! PRD-mcphost-session-bound-tenant-key
//! AC1 (P0) — Given a fresh connection on bare `/mcp`, When the first
//! `host.tool_publish` arrives without `tenant_key`, Then a tenant is
//! created, the onboarding payload is returned, and `created_session_id`
//! on the row is the connection's session id.
//!
//! This AC is a regression anchor, not a behavior change: a fresh
//! connection's first key-less call must keep implicitly signing up exactly
//! as `PRD-mcphost-implicit-signup`/`PRD-mcphost-session-bound-tenant-after-signup`
//! already landed. What this PRD changes is what happens on the SECOND
//! key-less call (`sessbind_ac15` and friends) -- never this first one.

use crate::common;
use common::{McpClient, TestServer, extract_structured};
use serde_json::json;

#[tokio::test]
async fn fresh_connections_first_bare_call_still_implicitly_signs_up() {
    let server = TestServer::start().await;
    let session = McpClient::new(&server.base_url).with_session_continuity();

    let publish = session
        .tools_call(
            "host.tool_publish",
            json!({
                "name": "hello",
                "kind": "echo",
                "spec": {"schema": {"type": "object", "properties": {}, "required": []}},
            }),
        )
        .await
        .expect("a bare host.tool_publish call must succeed, not fail tenant_key_missing");
    let published = extract_structured(&publish);

    let onboarding = &published["onboarding"];
    let tenant_ns = onboarding["tenant"]
        .as_str()
        .expect("onboarding.tenant present on an implicit first call")
        .to_string();

    let tenants = server.state.db.list_tenants().await.expect("list tenants");
    assert_eq!(tenants.len(), 1, "exactly one tenant must have been created");
    let row = &tenants[0];
    assert_eq!(row.namespace, tenant_ns);

    let server_session_id = session.session_id().expect("server issued a session id");
    assert_eq!(
        row.created_session_id.as_deref(),
        Some(server_session_id.as_str()),
        "created_session_id must be this connection's own server-issued session id: {row:?}"
    );
}
