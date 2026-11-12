//! PRD-mcphost-session-journal-v2
//! AC4 (P0) — Given tenant B calls tenant A's shared tool with arguments over
//! a session, When A runs `host.calls.list {credential: "shared_caller"}`,
//! Then that row has null `args_sha256` and null `session_id`; and a
//! `host.tool.publish` row of A's own made over an MCP session reads null
//! `args_sha256` and a non-null `session_id`.

use crate::common::{McpClient, TestServer, extract_structured, signup};
use serde_json::{Value, json};

async fn list(client: &McpClient, args: Value) -> Value {
    extract_structured(&client.tools_call("host.calls.list", args).await.expect("host.calls.list"))
}

#[tokio::test]
async fn jrnl_ac04_shared_caller_rows_carry_neither_value() {
    let server = TestServer::start().await;
    let (ns_a, key_a) = signup(&server.base_url, "Owner Tenant").await;
    // A works over an MCP session: a setup call issues the id, the publish
    // that follows carries it.
    let owner = McpClient::with_bearer(&server.base_url, &key_a).with_session_continuity();
    owner.tools_call("host.whoami", json!({})).await.expect("owner setup");
    // A host verb writes no `calls` row of its own; the row it does leave is
    // the idempotent replay's, so publish twice under one key.
    let publish = json!({
        "name": "geo", "kind": "echo", "spec": {"schema": {"type": "object"}},
        "__idempotency_key": "publish-geo-1",
    });
    owner.tools_call("host.tool.publish", publish.clone()).await.expect("A publishes geo");
    owner.tools_call("host.tool.publish", publish).await.expect("A's replayed publish");
    owner
        .tools_call("host.tool.share", json!({"name": "geo", "visibility": "public", "description": "geo lookup"}))
        .await
        .expect("A shares geo");
    let owner_session = owner.session_id().expect("owner session id");

    let (_ns_b, key_b) = signup(&server.base_url, "Caller Tenant").await;
    let caller = McpClient::with_bearer(&server.base_url, &key_b).with_session_continuity();
    caller.tools_call("host.whoami", json!({})).await.expect("caller setup");
    caller.tools_call(&format!("{ns_a}.geo"), json!({"msg": "hi"})).await.expect("B calls A's tool");
    assert!(caller.session_id().is_some(), "B called over a session");

    let shared = list(&owner, json!({"since": "-1h", "credential": "shared_caller"})).await;
    let rows = shared["calls"].as_array().expect("calls");
    assert_eq!(rows.len(), 1, "{shared}");
    assert!(rows[0]["args_sha256"].is_null(), "the owner never sees the caller's arguments: {shared}");
    assert!(rows[0]["session_id"].is_null(), "the owner never sees the caller's session: {shared}");

    let own = list(&owner, json!({"since": "-1h", "credential": "primary", "session": owner_session})).await;
    let publish = own["calls"]
        .as_array()
        .expect("calls")
        .iter()
        .find(|r| r["tool"].as_str().is_some_and(|t| t.ends_with("publish")))
        .unwrap_or_else(|| panic!("A's own publish row is listed: {own}"));
    assert!(publish["args_sha256"].is_null(), "a control-plane verb carries no fingerprint: {own}");
    assert_eq!(publish["session_id"], json!(owner_session), "{own}");
}
