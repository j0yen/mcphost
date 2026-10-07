//! PRD-mcphost-session-bound-tenant-key
//! AC4 (P0) — Given two concurrent connections each without keys, When each
//! makes its first call, Then two tenants are created with different
//! `created_session_id`s, and neither connection's later key-less call
//! creates a third.

use crate::common;
use common::{McpClient, TestServer, extract_structured};
use serde_json::json;

#[tokio::test]
async fn two_concurrent_bare_connections_each_get_their_own_tenant_and_no_third() {
    let server = TestServer::start_with_signup_rate_limit(100).await;

    let a = McpClient::new(&server.base_url).with_session_continuity();
    let b = McpClient::new(&server.base_url).with_session_continuity();

    let (a_result, b_result) = tokio::join!(
        a.tools_call("host.whoami", json!({})),
        b.tools_call("host.whoami", json!({})),
    );
    let a_ns = extract_structured(&a_result.expect("A's first bare call must succeed"))["tenant"]
        .as_str()
        .expect("A's namespace")
        .to_string();
    let b_ns = extract_structured(&b_result.expect("B's first bare call must succeed"))["tenant"]
        .as_str()
        .expect("B's namespace")
        .to_string();
    assert_ne!(a_ns, b_ns, "two concurrent connections must get two distinct tenants");

    let tenants = server.state.db.list_tenants().await.expect("list tenants");
    assert_eq!(tenants.len(), 2, "exactly two tenants exist");
    let a_row = tenants.iter().find(|t| t.namespace == a_ns).expect("A's row");
    let b_row = tenants.iter().find(|t| t.namespace == b_ns).expect("B's row");
    assert_ne!(
        a_row.created_session_id, b_row.created_session_id,
        "the two tenants must carry two different created_session_ids"
    );
    assert_ne!(a.session_id(), b.session_id());
    assert_eq!(a_row.created_session_id, a.session_id());
    assert_eq!(b_row.created_session_id, b.session_id());

    // A later key-less call on EITHER connection is refused and named --
    // naming its OWN tenant, never the other's -- and never a third.
    let (a_second, b_second) = tokio::join!(
        a.tools_call("host.usage", json!({})),
        b.tools_call("host.usage", json!({})),
    );
    let a_err = a_second.expect_err("A's second key-less call must be refused");
    assert_eq!(a_err.error_code.as_deref(), Some("tenant_key_missing"));
    assert_eq!(a_err.data.get("tenant").and_then(serde_json::Value::as_str), Some(a_ns.as_str()));
    let b_err = b_second.expect_err("B's second key-less call must be refused");
    assert_eq!(b_err.error_code.as_deref(), Some("tenant_key_missing"));
    assert_eq!(b_err.data.get("tenant").and_then(serde_json::Value::as_str), Some(b_ns.as_str()));

    let tenants = server.state.db.list_tenants().await.expect("list tenants");
    assert_eq!(tenants.len(), 2, "still exactly two tenants -- neither connection created a third");
}
