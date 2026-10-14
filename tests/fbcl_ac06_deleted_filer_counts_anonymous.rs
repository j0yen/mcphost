//! PRD-mcphost-feedback-closed-loop-notice AC6 — Given a filer whose tenant
//! was deleted, When the theme ships, Then the row is counted under
//! `anonymous` and the task does not error.

use crate::common;
use common::{ADMIN_KEY, FBCL_SECRET_TEXT, McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn a_deleted_filer_is_counted_anonymous_and_ship_succeeds() {
    let server = TestServer::start().await;
    let context = json!({"error_code": "args_invalid", "tool": "host.tool_call"});
    let mut namespaces = Vec::new();
    let mut clients = Vec::new();
    for name in ["fbcl6-a", "fbcl6-b"] {
        let (ns, key) = signup(&server.base_url, name).await;
        let c = McpClient::with_bearer(&server.base_url, &key);
        c.tools_call(
            "host.feedback.send",
            json!({"kind": "wrong_error", "text": FBCL_SECRET_TEXT, "context": context}),
        )
        .await
        .expect("feedback send");
        namespaces.push(ns);
        clients.push(c);
    }
    server.state.db.cluster_feedback().await.unwrap();
    let admin = McpClient::with_bearer(&server.base_url, ADMIN_KEY);
    let themes = extract_structured(&admin.tools_call("admin.feedback.themes", json!({})).await.unwrap());
    let theme_id = themes["themes"][0]["theme_id"].as_str().unwrap().to_string();

    admin
        .tools_call("admin.tenant_delete", json!({"tenant": namespaces[1].clone()}))
        .await
        .expect("admin.tenant_delete");

    let out = extract_structured(
        &admin
            .tools_call("admin.feedback.ship", json!({"theme_id": theme_id, "version": "0.110.0"}))
            .await
            .expect("ship must not error for a deleted filer"),
    );
    assert_eq!(out["notified"], 1, "{out}");
    assert_eq!(out["anonymous"], 1, "{out}");
    assert_eq!(out["already_notified"], 0, "{out}");
}
