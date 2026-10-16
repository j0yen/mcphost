//! PRD-mcphost-delegation-bound-to-run-or-session
//! AC1 (P0) — Given two MCP connections opened with the same tenant key, When
//! each calls `host.whoami`, Then both responses carry a non-empty, distinct
//! `session_id` in `_meta` (the falsifier), and a key delegated with `bind:
//! {session: "current"}` on connection A is refused `key_bound_elsewhere` on
//! connection B with `data.bound.kind == "session"` and `data.seen.ref` equal
//! to B's id.

use crate::common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn kbind_ac01_session_ids_distinct_and_session_bound_key_refused_on_other_connection() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Kbind Session Tenant").await;
    let a = McpClient::with_bearer(&server.base_url, &key).with_session_continuity();
    let b = McpClient::with_bearer(&server.base_url, &key).with_session_continuity();

    let who_a = a.tools_call("host.whoami", json!({})).await.expect("whoami a");
    let who_b = b.tools_call("host.whoami", json!({})).await.expect("whoami b");
    let sid_a = who_a["_meta"]["session_id"].as_str().expect("A _meta.session_id").to_string();
    let sid_b = who_b["_meta"]["session_id"].as_str().expect("B _meta.session_id").to_string();
    assert!(!sid_a.is_empty() && !sid_b.is_empty(), "{who_a} {who_b}");
    assert_ne!(sid_a, sid_b, "two connections must not share a session id");
    assert_eq!(Some(sid_a.clone()), a.session_id(), "_meta.session_id is the Mcp-Session-Id header");

    let delegated = extract_structured(
        &a.tools_call(
            "host.key.delegate",
            json!({"scopes": ["mcp"], "ttl_s": 600, "label": "sub-agent", "bind": {"session": "current"}}),
        )
        .await
        .expect("delegate bound to current session"),
    );
    let child_key = delegated["key"].as_str().expect("child key").to_string();

    // On A's own connection the bound key works.
    let on_a = McpClient::with_bearer(&server.base_url, &child_key).joining_session_of(&a);
    on_a.tools_call("host.tool.list", json!({})).await.expect("bound key works on its own connection");

    // On B it is refused, naming both sides.
    let on_b = McpClient::with_bearer(&server.base_url, &child_key).joining_session_of(&b);
    let err = on_b.tools_call("host.tool.list", json!({})).await.expect_err("refused on B");
    assert_eq!(err.error_code.as_deref(), Some("key_bound_elsewhere"), "{err:?}");
    assert_eq!(err.data["bound"]["kind"], json!("session"), "{err:?}");
    assert_eq!(err.data["bound"]["ref"], json!(sid_a), "{err:?}");
    assert_eq!(err.data["seen"]["ref"], json!(sid_b), "{err:?}");
}
