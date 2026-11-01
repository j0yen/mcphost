//! PRD-mcphost-implicit-signup-mints-only-on-success
//! AC1 (P0) — Given a keyless client on bare `/mcp`, When its first call is
//! `host.state.set` with arguments its schema rejects, or a `host.*` name
//! that does not exist, Then it gets `args_invalid` or `host_tool_not_found`,
//! the tenants table, `implicit_signup_memory`, `PeerRecents` and the signup
//! rate limiter are all unchanged, and a corrected retry creates exactly one
//! tenant with onboarding.

use crate::common;
use common::{McpClient, TestServer, extract_structured};
use serde_json::json;

async fn untouched(server: &TestServer) {
    let tenants = server.state.db.list_tenants().await.expect("list tenants");
    assert!(tenants.is_empty(), "a refused first call must mint no tenant: {tenants:?}");
    assert_eq!(server.state.implicit_signup_memory.len(), 0, "implicit_signup_memory is unchanged");
    assert!(server.state.redeemed_peers.is_empty(), "PeerRecents is unchanged");
    let charged = server.state.db.signup_count_since("127.0.0.1".to_string(), 0).await.expect("count");
    assert_eq!(charged, 0, "the signup rate limiter is not charged");
}

#[tokio::test]
async fn a_first_call_that_cannot_work_mints_nothing_and_the_corrected_retry_mints_one() {
    let server = TestServer::start().await;
    let client = McpClient::new(&server.base_url).with_user_agent("imos-ac01");

    let bad = client
        .tools_call("host.state.set", json!({"nope": 1}))
        .await
        .expect_err("a schema-rejected first call is refused");
    assert_eq!(bad.error_code.as_deref(), Some("args_invalid"), "{bad:?}");
    untouched(&server).await;

    let missing = client
        .tools_call("host.no_such_verb", json!({}))
        .await
        .expect_err("an unknown host.* name is refused");
    assert_eq!(missing.error_code.as_deref(), Some("tool_not_found") /* AppError::host_tool_not_found */, "{missing:?}");
    untouched(&server).await;

    let ok = client
        .tools_call("host.state.set", json!({"key": "k", "value": 1}))
        .await
        .expect("the corrected retry succeeds");
    let ok = extract_structured(&ok);
    assert!(ok["onboarding"]["url"].is_string(), "the retry carries onboarding: {ok}");
    let tenants = server.state.db.list_tenants().await.expect("list tenants");
    assert_eq!(tenants.len(), 1, "exactly one tenant after the corrected retry");
}
