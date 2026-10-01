//! PRD-mcphost-invite-links
//! AC13 (P0) — Given invitee B calling `<A ns>.t` for the first time in
//! a session, When the result returns, Then `_meta.invite_url` equals
//! B's standing invite URL, and the second call in the same session
//! carries no `_meta.invite_url`.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn shared_call_carries_invite_url_hint_once_per_session() {
    let server = TestServer::start().await;
    let (ns_a, key_a) = signup(&server.base_url, "AC13 A").await;
    let client_a = McpClient::with_bearer(&server.base_url, &key_a);

    client_a
        .tools_call(
            "host.tool_publish",
            json!({"name": "tt", "kind": "echo", "spec": {"schema": {"type": "object"}}}),
        )
        .await
        .expect("publish t");
    let created = client_a
        .tools_call("host.invite.create", json!({"share": ["tt"]}))
        .await
        .expect("host.invite.create");
    let url_a = extract_structured(&created)["url"].as_str().expect("url").to_string();
    let path_a = url_a.trim_start_matches(&server.base_url).to_string();

    let invitee = McpClient::new(&server.base_url)
        .with_path(&path_a)
        .with_session_continuity();
    // The session's first call must be a `host.*` call -- that's what
    // actually creates B's tenant and binds the session (requirement 2);
    // a raw `<ns>.local` call as the very first anonymous call on `/i/`
    // is not the trigger this PRD wires up. B's first SHARED-TOOL call
    // (what AC13 is actually about) is the one right after.
    invitee
        .tools_call("host.whoami", json!({}))
        .await
        .expect("B's first call joins via the invite");

    // B's first raw shared-tool call: the ONLY path this requirement's
    // `_meta` hint is wired on (see `handler::call_shared_tool`'s own doc
    // comment) -- `host.tool_call` is a separate call site with no
    // session identity on hand.
    let first = invitee
        .tools_call(&format!("{ns_a}.tt"), json!({}))
        .await
        .expect("B's first raw shared-tool call");
    let hint_url = first["_meta"]["invite_url"]
        .as_str()
        .expect("first shared call carries _meta.invite_url")
        .to_string();
    assert!(
        hint_url.starts_with(&format!("{}/i/", server.base_url)) && hint_url.ends_with("/mcp"),
        "{hint_url}"
    );

    let whoami_b = invitee.tools_call("host.whoami", json!({})).await.expect("host.whoami B");
    let own_standing = extract_structured(&whoami_b)["invite_url"]
        .as_str()
        .expect("B's own standing invite_url")
        .to_string();
    assert_eq!(hint_url, own_standing, "the hint must be B's OWN standing invite url");

    // Second call, same session: no hint.
    let second = invitee
        .tools_call(&format!("{ns_a}.tt"), json!({}))
        .await
        .expect("B's second raw shared-tool call");
    assert!(
        second.get("_meta").is_none_or(|m| m.get("invite_url").is_none()),
        "second call on the same session must carry no _meta.invite_url: {second}"
    );
}
