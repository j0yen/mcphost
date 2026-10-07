//! PRD-mcphost-session-bound-tenant-key
//! AC5 (P0) — Given a connection that signed up implicitly and then
//! disconnects, When a new connection without a key calls, Then a new
//! tenant is created (fresh client) and the old tenant is untouched.

use crate::common;
use common::{McpClient, TestServer, extract_structured};
use serde_json::json;

#[tokio::test]
async fn a_fresh_connection_after_disconnect_gets_its_own_new_tenant() {
    let server = TestServer::start().await;

    // The first connection implicitly signs up, then "disconnects" --
    // nothing carries its session id forward; the next McpClient below is
    // a genuinely independent connection, same as a real reconnect with no
    // continuity at all.
    let first_connection = McpClient::new(&server.base_url).with_session_continuity();
    let first = extract_structured(
        &first_connection
            .tools_call("host.whoami", json!({}))
            .await
            .expect("the first connection's bare call implicitly signs it up"),
    );
    let old_ns = first["tenant"].as_str().expect("old namespace").to_string();
    let old_session_id = first_connection.session_id().expect("server issued a session id");

    let tenants = server.state.db.list_tenants().await.expect("list tenants");
    assert_eq!(tenants.len(), 1);
    let old_row_before = tenants.iter().find(|t| t.namespace == old_ns).expect("old row").clone();

    // A brand-new connection: no session, no header, no tenant_key -- a
    // fresh client by every definition.
    let new_connection = McpClient::new(&server.base_url);
    let second = extract_structured(
        &new_connection
            .tools_call("host.whoami", json!({}))
            .await
            .expect("a genuinely fresh connection's bare call implicitly signs IT up too"),
    );
    let new_ns = second["tenant"].as_str().expect("new namespace").to_string();
    assert_ne!(new_ns, old_ns, "the fresh connection must get its OWN new tenant, never the old one");

    let tenants = server.state.db.list_tenants().await.expect("list tenants");
    assert_eq!(tenants.len(), 2, "a new tenant was created; the old one still exists");

    // The old tenant's own row is untouched -- same namespace, same
    // created_session_id, still the first connection's own session id.
    let old_row_after = tenants.iter().find(|t| t.namespace == old_ns).expect("old row still there");
    assert_eq!(old_row_after.created_session_id.as_deref(), Some(old_session_id.as_str()));
    assert_eq!(old_row_after.created_session_id, old_row_before.created_session_id);
    assert_eq!(old_row_after.namespace, old_row_before.namespace);
}
