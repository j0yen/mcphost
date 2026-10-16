//! PRD-mcphost-delegation-bound-to-run-or-session
//! AC5 (P0) — Given a session-bound key idle for 31 min (clock advanced in
//! test), When the sweep runs, Then it is revoked with `reason ==
//! "session_idle"`; Given the session is closed, Then revocation happens on
//! the close with `reason == "session_closed"`.

use crate::common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

async fn bound_child(server: &TestServer, conn: &McpClient, label: &str) -> McpClient {
    let out = extract_structured(
        &conn
            .tools_call("host.key.delegate", json!({"bind": {"session": "current"}, "label": label}))
            .await
            .unwrap_or_else(|e| panic!("delegate {label}: {e:?}")),
    );
    assert_eq!(out["bound"]["kind"], json!("session"), "{out}");
    McpClient::with_bearer(&server.base_url, out["key"].as_str().expect("key")).joining_session_of(conn)
}

#[tokio::test]
async fn kbind_ac05_idle_session_key_is_swept_and_a_closed_session_revokes_at_once() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Kbind Idle Tenant").await;
    let primary = McpClient::with_bearer(&server.base_url, &key);

    // Idle: a key bound to connection A, then 31 minutes of silence.
    let a = McpClient::with_bearer(&server.base_url, &key).with_session_continuity();
    let idle_child = bound_child(&server, &a, "idle-one").await;
    idle_child.tools_call("host.tool.list", json!({})).await.expect("works while fresh");

    let now = mcphost::state::now_unix();
    let early = server.state.db.sweep_idle_bound_sessions(now + 29 * 60).await.expect("early sweep");
    assert_eq!(early, 0, "29 minutes of silence is not idle yet");
    let swept = server.state.db.sweep_idle_bound_sessions(now + 31 * 60).await.expect("sweep");
    assert_eq!(swept, 1);
    let err = idle_child.tools_call("host.tool.list", json!({})).await.expect_err("swept key is dead");
    assert_eq!(err.error_code.as_deref(), Some("key_revoked"), "{err:?}");
    assert_eq!(err.data["reason"], json!("session_idle"), "{err:?}");

    // Closed: a key bound to connection C dies on the DELETE that ends C.
    let c = McpClient::with_bearer(&server.base_url, &key).with_session_continuity();
    let closed_child = bound_child(&server, &c, "closed-one").await;
    closed_child.tools_call("host.tool.list", json!({})).await.expect("works while open");
    let sid = c.session_id().expect("C session id");
    reqwest::Client::new()
        .delete(format!("{}/mcp", server.base_url))
        .header("Mcp-Session-Id", sid)
        .header("Authorization", format!("Bearer {key}"))
        .send()
        .await
        .expect("DELETE session");
    let err = closed_child.tools_call("host.tool.list", json!({})).await.expect_err("closed session's key is dead");
    assert_eq!(err.error_code.as_deref(), Some("key_revoked"), "{err:?}");
    assert_eq!(err.data["reason"], json!("session_closed"), "{err:?}");

    let listed = extract_structured(&primary.tools_call("host.key.list", json!({})).await.expect("list"));
    let reasons: Vec<_> = listed["keys"]
        .as_array()
        .expect("keys")
        .iter()
        .map(|k| (k["label"].as_str().unwrap_or("").to_string(), k["revoked_reason"].clone()))
        .collect();
    assert!(reasons.contains(&("idle-one".into(), json!("session_idle"))), "{reasons:?}");
    assert!(reasons.contains(&("closed-one".into(), json!("session_closed"))), "{reasons:?}");
}
