//! PRD-mcphost-session-journal-v2
//! AC3 (P0) — Given two MCP sessions on one tenant with 3 and 2 calls of one
//! published tool and one stateless call of it, When `host.calls.list
//! {session, tool}` runs for the first id (the `tool` filter leaves out the
//! session's setup and control-plane rows, which also carry the id), Then 3
//! rows return, each with that `session_id`; for another tenant's session id
//! it returns `calls: []`; the stateless row reads `session_id: null`.

use crate::common::{McpClient, TestServer, extract_structured, signup};
use serde_json::{Value, json};

async fn list(client: &McpClient, args: Value) -> Value {
    extract_structured(&client.tools_call("host.calls.list", args).await.expect("host.calls.list"))
}

#[tokio::test]
async fn jrnl_ac03_session_id_stamps_rows_and_filters_the_list() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "Session Tenant").await;
    let stateless = McpClient::with_bearer(&server.base_url, &key);
    stateless
        .tools_call(
            "host.tool.publish",
            json!({"name": "sess_echo", "kind": "echo", "spec": {"schema": {"type": "object"}}}),
        )
        .await
        .expect("publish");
    let tool = format!("{ns}.sess_echo");

    let first = McpClient::with_bearer(&server.base_url, &key).with_session_continuity();
    let second = McpClient::with_bearer(&server.base_url, &key).with_session_continuity();
    // The host issues an id on a session's first request and a client echoes
    // it from the second on, so each session opens with a setup call (a
    // control-plane row that carries the id too; the `tool` filter skips it).
    first.tools_call("host.whoami", json!({})).await.expect("first session setup");
    second.tools_call("host.whoami", json!({})).await.expect("second session setup");
    for i in 0..3 {
        first.tools_call(&tool, json!({"n": i})).await.expect("first session call");
    }
    for i in 0..2 {
        second.tools_call(&tool, json!({"n": i})).await.expect("second session call");
    }
    stateless.tools_call(&tool, json!({"n": 9})).await.expect("stateless call");
    let first_id = first.session_id().expect("the first session was issued an id");
    let second_id = second.session_id().expect("the second session was issued an id");
    assert_ne!(first_id, second_id);

    let only_first = list(&stateless, json!({"since": "-1h", "tool": "sess_echo", "session": first_id})).await;
    let rows = only_first["calls"].as_array().expect("calls");
    assert_eq!(rows.len(), 3, "{only_first}");
    for row in rows {
        assert_eq!(row["session_id"], json!(first_id), "{only_first}");
    }
    let only_second = list(&stateless, json!({"since": "-1h", "tool": "sess_echo", "session": second_id})).await;
    assert_eq!(only_second["calls"].as_array().expect("calls").len(), 2, "{only_second}");

    // The stateless call carries no session id.
    let all = list(&stateless, json!({"since": "-1h", "tool": "sess_echo"})).await;
    let rows = all["calls"].as_array().expect("calls");
    assert_eq!(rows.len(), 6, "{all}");
    assert_eq!(rows.iter().filter(|r| r["session_id"].is_null()).count(), 1, "{all}");
    assert!(rows[0]["session_id"].is_null(), "newest row is the stateless call: {all}");

    // A session id that belongs to another tenant matches nothing, in either direction.
    let (_other_ns, other_key) = signup(&server.base_url, "Other Session Tenant").await;
    let other = McpClient::with_bearer(&server.base_url, &other_key).with_session_continuity();
    other.tools_call("host.whoami", json!({})).await.expect("other tenant call");
    let other_id = other.session_id().expect("other session id");
    let foreign = list(&stateless, json!({"since": "-1h", "session": other_id})).await;
    assert_eq!(foreign["calls"], json!([]), "{foreign}");
    let reverse = list(&other, json!({"since": "-1h", "session": first_id})).await;
    assert_eq!(reverse["calls"], json!([]), "{reverse}");
}
