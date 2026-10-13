//! PRD-mcphost-feedback-themes-nightly AC7 — Given a tenant that filed `f_a`
//! and another that filed `f_b`, When the first calls `host.feedback.get
//! {feedback_id:"f_b"}`, Then the code is `feedback_not_found`; `f_a` returns
//! `theme_id` and `status: "open"` after clustering.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

async fn send(client: &McpClient, text: &str) -> String {
    let res = client
        .tools_call(
            "host.feedback.send",
            json!({"kind": "wrong_error", "text": text, "context": {"error_code": "args_invalid", "tool": "host.tool_call"}}),
        )
        .await
        .expect("send");
    extract_structured(&res)["feedback_id"].as_str().unwrap().to_string()
}

#[tokio::test]
async fn get_is_owner_only_and_carries_the_theme_after_clustering() {
    let server = TestServer::start().await;
    let (_ns_a, key_a) = signup(&server.base_url, "fbth-a").await;
    let (_ns_b, key_b) = signup(&server.base_url, "fbth-b").await;
    let a = McpClient::with_bearer(&server.base_url, &key_a);
    let b = McpClient::with_bearer(&server.base_url, &key_b);
    let f_a = send(&a, "args_invalid said 4.78 is not an integer").await;
    let f_b = send(&b, "args_invalid says 12.5 is not an integer").await;

    // Someone else's id, and an id that never existed, are the same answer.
    for id in [f_b.as_str(), "f_does_not_exist"] {
        let err = a
            .tools_call("host.feedback.get", json!({"feedback_id": id}))
            .await
            .expect_err("a caller cannot read a row it does not own");
        assert_eq!(err.error_code.as_deref(), Some("feedback_not_found"), "{id}");
    }
    // An anonymous caller owns nothing.
    let anon = McpClient::new(&server.base_url);
    let err = anon
        .tools_call("host.feedback.get", json!({"feedback_id": f_a}))
        .await
        .expect_err("anonymous get");
    assert_eq!(err.error_code.as_deref(), Some("feedback_not_found"));

    // Before the nightly pass: own row, no theme yet.
    let before = extract_structured(&a.tools_call("host.feedback.get", json!({"feedback_id": f_a})).await.unwrap());
    assert!(before["theme_id"].is_null());
    assert_eq!(before["status"], "received");

    server.state.db.cluster_feedback().await.unwrap();
    let after = extract_structured(&a.tools_call("host.feedback.get", json!({"feedback_id": f_a})).await.unwrap());
    let theme_id = after["theme_id"].as_str().expect("theme_id after clustering");
    assert!(theme_id.starts_with("th_"));
    assert_eq!(after["status"], "open");
    assert_eq!(after["feedback_id"], json!(f_a));
    // Both tenants' variants share the theme, but each reads only its own row.
    let theirs = server.state.db.feedback_row_theme_id(f_b).await.unwrap();
    assert_eq!(theirs.as_deref(), Some(theme_id));
}
