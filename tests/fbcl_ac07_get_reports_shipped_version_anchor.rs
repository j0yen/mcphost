//! PRD-mcphost-feedback-closed-loop-notice AC7 — Given a shipped theme, When
//! the filer calls `host.feedback.get`, Then `status: "shipped"`, `version`,
//! `anchor` are present and `FeedbackStatus` has exactly `received, themed,
//! shipped`.

use crate::common;
use common::{ADMIN_KEY, McpClient, TestServer, extract_structured, fbcl_themed};
use mcphost::feedback::FeedbackStatus;
use serde_json::json;

#[tokio::test]
async fn get_reports_status_version_and_anchor_once_shipped() {
    let names: Vec<&str> = FeedbackStatus::ALL.iter().map(|s| s.as_str()).collect();
    assert_eq!(names, ["received", "themed", "shipped"]);

    let server = TestServer::start().await;
    let (clients, _ids, theme_id) = fbcl_themed(&server, &["fbcl7-a"], 0).await;
    let c = &clients[0];
    let list = extract_structured(&c.tools_call("host.feedback.list", json!({})).await.unwrap());
    let feedback_id = list["feedback"][0]["feedback_id"].as_str().unwrap().to_string();
    assert_eq!(list["feedback"][0]["status"], "themed");
    let before = extract_structured(&c.tools_call("host.feedback.get", json!({"feedback_id": feedback_id})).await.unwrap());
    assert_eq!(before["status"], "themed");
    assert!(before.get("version").is_none(), "{before}");

    let admin = McpClient::with_bearer(&server.base_url, ADMIN_KEY);
    admin
        .tools_call("admin.feedback.ship", json!({"theme_id": theme_id, "version": "0.110.0", "anchor": "args"}))
        .await
        .unwrap();
    let got = extract_structured(&c.tools_call("host.feedback.get", json!({"feedback_id": feedback_id})).await.unwrap());
    assert_eq!(got["status"], "shipped", "{got}");
    assert_eq!(got["version"], "0.110.0");
    assert_eq!(got["anchor"], "args");
    let list = extract_structured(&c.tools_call("host.feedback.list", json!({})).await.unwrap());
    assert_eq!(list["feedback"][0]["status"], "shipped");
    assert_eq!(list["feedback"][0]["version"], "0.110.0");
}
