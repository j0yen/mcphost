//! PRD-mcphost-feedback-closed-loop-notice AC5 — Given any tenant, When it
//! calls `host.msg.send {to:["@mcphost"], body:"x"}`, Then the code is
//! `forbidden` and `data.hint` contains `host.feedback.send`.

use crate::common;
use common::{ADMIN_KEY, McpClient, TestServer, extract_structured, fbcl_themed};
use serde_json::json;

#[tokio::test]
async fn sending_to_the_system_handle_is_forbidden_with_a_hint() {
    let server = TestServer::start().await;
    let (clients, _ids, theme_id) = fbcl_themed(&server, &["fbcl5-a"], 0).await;
    let c = &clients[0];
    for to in ["@mcphost", "@McpHost"] {
        let err = c
            .tools_call("host.msg.send", json!({"to": [to], "body": "x"}))
            .await
            .expect_err("a tenant cannot message the host sender");
        assert_eq!(err.error_code.as_deref(), Some("forbidden"), "{to}");
        let hint = err.data["hint"].as_str().unwrap_or_default().to_string();
        assert!(hint.contains("host.feedback.send"), "hint: {hint}");
    }

    // A reply on a notice thread is refused the same way.
    let admin = McpClient::with_bearer(&server.base_url, ADMIN_KEY);
    admin.tools_call("admin.feedback.ship", json!({"theme_id": theme_id, "version": "0.110.0"})).await.unwrap();
    let inbox = extract_structured(&c.tools_call("host.msg.inbox", json!({})).await.unwrap());
    let thread_id = inbox["messages"][0]["thread_id"].as_str().unwrap().to_string();
    let err = c
        .tools_call("host.msg.reply", json!({"thread_id": thread_id, "body": "thanks"}))
        .await
        .expect_err("a reply to @mcphost");
    assert_eq!(err.error_code.as_deref(), Some("forbidden"));
}
