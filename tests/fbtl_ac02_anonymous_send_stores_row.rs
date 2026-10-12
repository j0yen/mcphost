//! PRD-mcphost-feedback-tool-and-ledger AC2 — Given an anonymous connection,
//! When it calls `host.feedback.send {kind:"missing_tool", text:"x"}`, Then
//! the result carries `feedback_id` matching `^f_[0-9a-f]{16}$` and a
//! `feedback` row exists with `tenant_id` null and `client_name` set.

use crate::common;
use common::{McpClient, TestServer, extract_structured};
use serde_json::json;

#[tokio::test]
async fn anonymous_send_returns_id_and_stores_a_tenantless_row() {
    let server = TestServer::start().await;
    let client = McpClient::new(&server.base_url).with_client_info("fbtl-agent", "9.9");

    let res = client
        .tools_call("host.feedback.send", json!({"kind": "missing_tool", "text": "x"}))
        .await
        .expect("an anonymous send must succeed");
    let body = extract_structured(&res);
    let id = body["feedback_id"].as_str().expect("feedback_id in the result");
    assert!(
        id.len() == 18
            && id.starts_with("f_")
            && id[2..].chars().all(|c| matches!(c, '0'..='9' | 'a'..='f')),
        "feedback_id must match ^f_[0-9a-f]{{16}}$, got {id}"
    );
    assert_eq!(body["kind"], "missing_tool");

    let row = server
        .state
        .db
        .get_feedback(id.to_string())
        .await
        .unwrap()
        .expect("the feedback row exists");
    assert_eq!(row.tenant_id, None, "an anonymous sender has no tenant");
    assert_eq!(row.client_name.as_deref(), Some("fbtl-agent"));
    assert_eq!(row.text, "x");
    assert_eq!(row.kind, "missing_tool");

    // Anonymous send must not implicitly sign the connection up.
    assert!(server.state.db.list_tenants().await.unwrap().is_empty());
}
