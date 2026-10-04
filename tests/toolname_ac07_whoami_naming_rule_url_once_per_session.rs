//! PRD-mcphost-tool-naming-convention-and-aliases
//! AC7 — Given a new session's first `host.whoami`, When answered, Then
//! the response includes `naming_rule_url`; the second call in the
//! session does not.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};

#[tokio::test]
async fn first_whoami_in_a_session_carries_the_hint_second_does_not() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "AC7 Tenant").await;
    let session = McpClient::with_bearer(&server.base_url, &key).with_session_continuity();

    let first = session.tools_call("host.whoami", serde_json::json!({})).await.expect("first whoami");
    let first_structured = extract_structured(&first);
    assert_eq!(
        first_structured["naming_rule_url"].as_str().map(|s| s.ends_with("docs/tool-naming.md")),
        Some(true),
        "the first host.whoami in a session must carry naming_rule_url: {first_structured}"
    );

    let second = session.tools_call("host.whoami", serde_json::json!({})).await.expect("second whoami");
    let second_structured = extract_structured(&second);
    assert!(
        second_structured.get("naming_rule_url").is_none(),
        "the second host.whoami in the same session must not repeat naming_rule_url: {second_structured}"
    );
}

/// A different session gets the hint again -- it's per-session, not
/// per-tenant.
#[tokio::test]
async fn a_different_session_gets_the_hint_again() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "AC7 Tenant Two").await;

    let session_a = McpClient::with_bearer(&server.base_url, &key).with_session_continuity();
    session_a.tools_call("host.whoami", serde_json::json!({})).await.expect("session A whoami");

    let session_b = McpClient::with_bearer(&server.base_url, &key).with_session_continuity();
    let first_b = session_b.tools_call("host.whoami", serde_json::json!({})).await.expect("session B whoami");
    let structured_b = extract_structured(&first_b);
    assert!(
        structured_b.get("naming_rule_url").is_some(),
        "a different session's first host.whoami must still carry naming_rule_url: {structured_b}"
    );
}
