//! PRD-mcphost-shared-tool-call-path
//! AC7 — Given O's own call host.tool_call {name: "lookup"}, When it runs
//! before and after this change on the existing fixture, Then the response
//! is byte-identical.
//!
//! "Before this change" is proven by pinning the exact expected response
//! shape (same fixture convention `sessionkey_ac16_full_session_no_header.rs`
//! already uses for host.tool_call's own-tool path: publish, then
//! `extract_structured(&call) == json!({...})`) -- `call_published_tool`
//! returns the echo kind's own output untouched, no extra fields. Two
//! consecutive calls are also compared against each other, proving the new
//! `name.split_once('.')` routing this PRD adds produces byte-identical
//! output for the unqualified (`None`) branch on repeat, not just once.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn own_unqualified_host_tool_call_response_is_unchanged() {
    let server = TestServer::start().await;

    let (_ns_o, key_o) = signup(&server.base_url, "Owner").await;
    let client_o = McpClient::with_bearer(&server.base_url, &key_o);
    client_o
        .tools_call(
            "host.tool_publish",
            json!({"name": "lookup", "kind": "echo", "spec": {"schema": {"type": "object"}}}),
        )
        .await
        .expect("O publishes lookup");

    let expected = json!({"msg": "hi"});

    let call1 = client_o
        .tools_call("host.tool_call", json!({"name": "lookup", "args": {"msg": "hi"}}))
        .await
        .expect("O calls its own unqualified tool");
    let result1 = extract_structured(&call1);
    assert_eq!(result1, expected, "own unqualified host.tool_call response shape must be unchanged");

    let call2 = client_o
        .tools_call("host.tool_call", json!({"name": "lookup", "args": {"msg": "hi"}}))
        .await
        .expect("O calls its own unqualified tool again");
    let result2 = extract_structured(&call2);
    assert_eq!(result1, result2, "repeat calls of the unqualified path must stay byte-identical to each other");
}
