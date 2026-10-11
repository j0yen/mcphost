//! PRD-mcphost-ask-user-decision
//! AC5 -- Given a key-less `host.ask.wait` on bare `/mcp`, When it is
//! called, Then it returns `tenant_key_missing` in under 1 s and no tenant
//! row is created.

use crate::common;

use common::{McpClient, TestServer};
use serde_json::json;
use std::time::{Duration, Instant};

#[tokio::test]
async fn keyless_ask_wait_is_refused_fast_and_creates_no_tenant() {
    let server = TestServer::start().await;
    assert_eq!(server.state.db.list_tenants().await.expect("list tenants").len(), 0);

    let started = Instant::now();
    let err = McpClient::new(&server.base_url)
        .tools_call("host.ask.wait", json!({"ask_id": "01hxnosuchask", "timeout_s": 25}))
        .await
        .expect_err("a key-less host.ask.wait must be refused");
    assert!(started.elapsed() < Duration::from_secs(1), "refusal took {:?}", started.elapsed());
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
