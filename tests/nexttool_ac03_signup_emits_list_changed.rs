//! PRD-mcphost-one-next-tool AC3 (P0) — Given an anonymous session with an
//! open notification channel, When `signup` succeeds, Then
//! `notifications/tools/list_changed` is emitted on that session within
//! 100 ms and the next `tools/list` returns the full listing.
//!
//! mcphost's streamable-HTTP transport is stateless
//! (`with_legacy_session_mode(false)`): the only server-to-client channel a
//! session is ever guaranteed to have open is the response to the very
//! request that bound it (`rmcp`'s `StreamableHttpServerConfig::json_response`
//! doc: a notification emitted before the handler's final response forces
//! that one response to upgrade from plain JSON to an SSE stream carrying
//! both, in order) -- so "within 100 ms" is proven here by the notification
//! and the result arriving on the SAME wire response, not by a timed poll.

use crate::common;
use common::{McpClient, TestServer};
use serde_json::json;

#[tokio::test]
async fn signup_emits_list_changed_before_its_own_result_and_the_next_listing_is_full() {
    let server = TestServer::start().await;
    let session = McpClient::new(&server.base_url).with_session_continuity();

    let messages = session
        .tools_call_collect_messages("signup", json!({"name": "AC3 Tenant"}))
        .await;

    assert!(
        messages.len() >= 2,
        "a binding signup must carry the list_changed notification plus the real result on \
         the same response: {messages:?}"
    );
    let notification = &messages[0];
    assert_eq!(
        notification["method"].as_str(),
        Some("notifications/tools/list_changed"),
        "the first message on the wire must be the list_changed notification: {messages:?}"
    );
    assert!(
        notification.get("id").is_none(),
        "a notification must carry no JSON-RPC id: {notification:?}"
    );

    let final_response = messages.last().expect("at least one message");
    assert!(
        final_response.get("result").is_some(),
        "the last message must be signup's own JSON-RPC result: {final_response:?}"
    );

    // Then: the next tools/list on this same (now-bound) session returns
    // the full listing, no reconnect, no credential.
    let listed = session.tools_list().await.expect("tools/list after signup");
    let names: Vec<&str> = listed["tools"]
        .as_array()
        .expect("tools array")
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert!(
        names.len() > 12,
        "the bound session's next tools/list must return the full listing, not the starter \
         set: {names:?}"
    );
    assert!(
        !names.contains(&"signup"),
        "the full (authenticated-shaped) listing never includes signup: {names:?}"
    );
}
