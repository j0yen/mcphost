//! PRD-mcphost-one-next-tool AC2 (P0) — Given a session authenticated by
//! `tenant_key`, `Authorization` header, or `/u/<secret>/mcp`, When
//! `tools/list` is called, Then the listing is byte-identical to the
//! pre-PRD full listing at the same commit.
//!
//! `/u/<secret>/mcp` is the personal-URL credential
//! PRD-mcphost-url-bound-tenants would mint; that PRD has not landed in
//! this repo (mcphost-one-next-tool is explicitly independent of it per
//! the vision's own Order section), so this proves the two auth paths that
//! exist today: the `Authorization` header, and the session-binding
//! `signup`/`host.redeem` already mint (the PRD's own effective stand-in
//! for "authenticated by tenant_key" on a connection that carries no
//! header at all). Both are asserted byte-identical to
//! `handler::host_tool_descriptors` -- the one, untouched, pre-PRD source
//! of the full listing -- and to each other.

use crate::common;
use common::{McpClient, TestServer, signup};
use mcphost::handler::host_tool_descriptors;
use mcphost::kinds::KindRegistry;
use serde_json::Value;

/// The full listing's own tool array, independent of any particular
/// tenant's published tools -- built the exact same way `list_tools`'s
/// `Auth::Tenant` branch does for a tenant with zero of its own, so this is
/// what a freshly-signed-up tenant's `tools/list` must match byte for byte.
fn expected_full_listing() -> Value {
    let kinds = KindRegistry::with_builtin();
    serde_json::to_value(host_tool_descriptors(&kinds)).expect("serialize descriptors")
}

#[tokio::test]
async fn header_authenticated_listing_matches_the_pre_prd_full_listing() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "AC2 Header Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let listed = client.tools_list().await.expect("header tools/list");
    let tools = listed["tools"].clone();

    assert_eq!(
        tools, expected_full_listing(),
        "a header-authenticated tools/list must be byte-identical to the pre-PRD full listing"
    );
}

#[tokio::test]
async fn session_bound_listing_matches_the_pre_prd_full_listing_and_the_header_path() {
    let server = TestServer::start().await;
    let session = McpClient::new(&server.base_url).with_session_continuity();

    session
        .tools_call("signup", serde_json::json!({"name": "AC2 Bound Tenant"}))
        .await
        .expect("signup");

    // Keyless on purpose: the session's own binding (requirement 2/3) is
    // what must resolve this to the full listing, not an Authorization
    // header or a tenant_key argument.
    let listed = session.tools_list().await.expect("bound tools/list");
    let tools = listed["tools"].clone();

    assert_eq!(
        tools, expected_full_listing(),
        "a session bound by signup must see a tools/list byte-identical to the pre-PRD full \
         listing, with no reconnect and no credential on this call"
    );
}
