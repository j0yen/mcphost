//! PRD-mcphost-tool-naming-convention-and-aliases
//! AC7 (P1) -- Given a new session's first `host.whoami`, When answered,
//! Then the response includes `naming_rule_url`; the second call in the
//! session does not.

use crate::common;
use common::{McpClient, TestServer};
use serde_json::json;

#[tokio::test]
async fn first_whoami_in_a_session_carries_naming_rule_url_second_does_not() {
    let server = TestServer::start().await;
    let (_tenant, key) = common::signup(&server.base_url, "ac07-caller").await;
    // `with_session_continuity` negotiates and reuses one `Mcp-Session-Id`
    // across calls on this client -- without it every call would be its
    // own "session" and both calls would read as "first".
    let authed = McpClient::with_bearer(&server.base_url, &key).with_session_continuity();

    let first = authed.tools_call("host.whoami", json!({})).await.expect("first whoami");
    let first_structured = common::extract_structured(&first);
    assert_eq!(
        first_structured["naming_rule_url"],
        mcphost::tool_aliases::NAMING_RULE_URL,
        "{first_structured}"
    );

    let second = authed.tools_call("host.whoami", json!({})).await.expect("second whoami");
    let second_structured = common::extract_structured(&second);
    assert!(
        second_structured.get("naming_rule_url").is_none(),
        "second call in the same session must not repeat naming_rule_url: {second_structured}"
    );
}

#[tokio::test]
async fn a_fresh_session_gets_naming_rule_url_again() {
    let server = TestServer::start().await;
    let (_tenant, key) = common::signup(&server.base_url, "ac07-caller-2").await;

    let session_a = McpClient::with_bearer(&server.base_url, &key).with_session_continuity();
    let a1 = session_a.tools_call("host.whoami", json!({})).await.expect("session a, call 1");
    assert!(common::extract_structured(&a1).get("naming_rule_url").is_some());

    let session_b = McpClient::with_bearer(&server.base_url, &key).with_session_continuity();
    let b1 = session_b.tools_call("host.whoami", json!({})).await.expect("session b, call 1");
    assert!(
        common::extract_structured(&b1).get("naming_rule_url").is_some(),
        "a different session's first whoami gets the hint too"
    );
}
