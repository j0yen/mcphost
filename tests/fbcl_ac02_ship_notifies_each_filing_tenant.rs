//! PRD-mcphost-feedback-closed-loop-notice AC2 — Given a theme with 3 filing
//! tenants and 2 anonymous rows, When `admin.feedback.ship` runs, Then it
//! returns `{notified:3, anonymous:2, already_notified:0}` and each tenant's
//! `host.msg.inbox {unread_only:true}` holds one message from `@mcphost` whose
//! body names the version and anchor and contains none of the feedback text.

use crate::common;
use common::{ADMIN_KEY, McpClient, TestServer, extract_structured, fbcl_themed};
use serde_json::json;

#[tokio::test]
async fn ship_notifies_each_filing_tenant_once_and_names_no_feedback_text() {
    let server = TestServer::start().await;
    let (clients, _tenant_ids, theme_id) = fbcl_themed(&server, &["fbcl-a", "fbcl-b", "fbcl-c"], 2).await;
    let admin = McpClient::with_bearer(&server.base_url, ADMIN_KEY);
    let res = admin
        .tools_call("admin.feedback.ship", json!({"theme_id": theme_id, "version": "0.110.0", "anchor": "args"}))
        .await
        .expect("ship");
    let out = extract_structured(&res);
    assert_eq!(out, json!({"notified": 3, "anonymous": 2, "already_notified": 0}));

    for c in &clients {
        let inbox = extract_structured(&c.tools_call("host.msg.inbox", json!({"unread_only": true})).await.unwrap());
        let msgs = inbox["messages"].as_array().unwrap_or_else(|| panic!("messages array: {inbox}"));
        assert_eq!(msgs.len(), 1, "{inbox}");
        let m = &msgs[0];
        assert_eq!(m["from_address"], "@mcphost", "{m}");
        let body = m["body"].as_str().unwrap();
        assert!(body.contains("0.110.0") && body.contains("#args"), "{body}");
        assert!(body.contains(&theme_id), "{body}");
        assert!(!body.contains("SECRET-FEEDBACK-MARKER") && !body.contains("integer"), "{body}");
    }
}
