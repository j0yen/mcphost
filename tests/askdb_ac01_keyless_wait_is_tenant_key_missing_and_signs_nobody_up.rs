//! PRD-mcphost-ask-user-decision-b
//! AC1 -- Given a key-less `host.ask.wait` on bare `/mcp`, When it is
//! called, Then it returns `tenant_key_missing` in under 1 s and no tenant
//! row is created.

use crate::common;

use common::{McpClient, TestServer};
use serde_json::json;
use std::time::Duration;

#[tokio::test]
async fn keyless_ask_wait_is_refused_fast_and_creates_no_tenant() {
    let server = TestServer::start().await;
    assert_eq!(server.state.db.list_tenants().await.expect("list tenants").len(), 0);

    // "Under 1 s" is the timeout itself: a verb that blocked for its 25 s
    // ceiling, or signed up and went on to wait, would fail here.
    let err = tokio::time::timeout(
        Duration::from_secs(1),
        McpClient::new(&server.base_url).tools_call("host.ask.wait", json!({"ask_id": "01hxnosuchask", "timeout_s": 25})),
    )
    .await
    .expect("a key-less host.ask.wait must be refused in under 1 s")
    .expect_err("a key-less host.ask.wait must be refused");
    assert_eq!(err.error_code.as_deref(), Some("tenant_key_missing"), "{err:?}");

    assert_eq!(
        server.state.db.list_tenants().await.expect("list tenants").len(),
        0,
        "host.ask.wait must never sign a tenant up"
    );

    // Control: the same bare-/mcp peer still gets implicit signup on a verb
    // that is allowed to create one, so the refusal above is the verb's own.
    McpClient::new(&server.base_url)
        .tools_call("host.whoami", json!({}))
        .await
        .expect("an ordinary host.* call keeps its implicit signup");
    assert_eq!(server.state.db.list_tenants().await.expect("list tenants").len(), 1);
}
