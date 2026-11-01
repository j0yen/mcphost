//! PRD-mcphost-implicit-signup-per-session-not-per-peer
//! AC2 (P0) -- Given session A of AC 1, When A calls `host.table.create`
//! again without `tenant_key` inside the window, Then the call succeeds as
//! A's tenant through the session binding (the call row's auth method is
//! `session`), no second tenant is created, and no `tenant_key_missing`
//! error is produced (the gate is never reached by the redeeming session).

use crate::common;
use common::{McpClient, TestServer, extract_structured};
use serde_json::json;

#[tokio::test]
async fn ipsess_ac02() {
    let server = TestServer::start().await;
    let a = McpClient::new(&server.base_url).with_user_agent("ipsess-ac02/agent").with_session_continuity();
    a.initialize().await;
    let signup = extract_structured(
        &a.tools_call("signup", json!({"name": "ipsess-a", "handoff": true})).await.expect("A signup"),
    );
    let namespace = signup["tenant"].as_str().expect("tenant").to_string();
    let token = signup["handoff_token"].as_str().expect("handoff_token").to_string();
    a.tools_call("host.redeem", json!({"handoff_token": token})).await.expect("A redeem");

    let created = a
        .tools_call("host.table.create", json!({"name": "runs", "columns": {"id": "integer"}}))
        .await
        .expect("A's keyless call runs as A through the session binding, never as a refusal");
    assert!(extract_structured(&created).get("onboarding").is_none(), "no implicit signup: {created}");
    assert_eq!(server.state.db.list_tenants().await.expect("tenants").len(), 1, "no second tenant");

    // `host.table.create` writes no `calls` row (only tool executions do), so the
    // session-bound metering is read off a tool call made the same keyless way.
    a.tools_call(
        "host.tool_publish",
        json!({"name": "ac02tool", "kind": "echo", "spec": {"schema": {"type": "object", "properties": {}, "required": []}}}),
    )
    .await
    .expect("A publishes keyless through the session binding");
    a.tools_call(&format!("{namespace}.ac02tool"), json!({})).await.expect("A calls its tool keyless");
    assert_eq!(server.state.db.list_tenants().await.expect("tenants").len(), 1, "still no second tenant");

    let conn = rusqlite::Connection::open(server.data_dir.0.join("mcphost.db")).expect("open raw db");
    let tenant_id: i64 = conn
        .query_row("SELECT id FROM tenants WHERE namespace = ?1", [&namespace], |r| r.get(0))
        .expect("A's tenant id");
    let auth_method: String = conn
        .query_row(
            "SELECT auth_method FROM calls WHERE tenant_id = ?1 ORDER BY id DESC LIMIT 1",
            [tenant_id],
            |r| r.get(0),
        )
        .expect("the keyless tool call wrote a calls row for A");
    assert_eq!(auth_method, "session", "the call row's auth method");
}
