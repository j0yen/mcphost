//! PRD-mcphost-client-attribution-default-leak
//! AC2 — Given a request that DOES send proper MCP `clientInfo` (e.g. a
//! real `initialize` handshake from an SDK client), When the tenant record
//! is read back, Then the real reported client name/version are captured
//! unchanged (no regression on the `peer_client_info` fix's fallback
//! path -- see `attribdefault_ac1_bare_signup_client_info_is_null.rs` and
//! `src/handler.rs`'s `peer_client_info`).

use crate::common;
use common::{McpClient, extract_structured};
use serde_json::json;

#[tokio::test]
async fn real_clientinfo_from_initialize_handshake_is_captured_unchanged() {
    let server = common::TestServer::start().await;
    let client = McpClient::new(&server.base_url).with_client_info("acme-agent", "9.9.9");

    // A real `initialize` handshake, then a subsequent stateless call that
    // relies on `RequestContext::client_info()`'s fallback to the
    // session's persisted peer info -- exactly the path `peer_client_info`
    // must keep trusting when it genuinely differs from rmcp's own
    // synthesized placeholder identity.
    client.initialize().await;

    let signup_result = client
        .tools_call("signup", json!({"name": "Real ClientInfo Agent"}))
        .await
        .expect("signup");
    let ns = extract_structured(&signup_result)["tenant"]
        .as_str()
        .expect("tenant field")
        .to_string();

    let tenant = server
        .state
        .db
        .find_tenant_by_namespace(ns)
        .await
        .expect("query")
        .expect("tenant exists");
    assert_eq!(tenant.client_name.as_deref(), Some("acme-agent"));
    assert_eq!(tenant.client_version.as_deref(), Some("9.9.9"));
}
