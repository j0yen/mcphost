//! PRD-mcphost-feedback-tool-and-ledger AC3 — Given `text` of 2049 bytes or
//! containing `\x00`, When sent, Then the oversize call returns
//! `invalid_params` with `data.max_bytes: 2048` and the control-char call
//! stores the text with `\x00` removed.

use crate::common;
use common::{McpClient, TestServer, extract_structured};
use serde_json::json;

#[tokio::test]
async fn oversize_text_is_invalid_params_with_max_bytes() {
    let server = TestServer::start().await;
    let client = McpClient::new(&server.base_url);

    let err = client
        .tools_call("host.feedback.send", json!({"kind": "bug", "text": "a".repeat(2049)}))
        .await
        .expect_err("2049 bytes must be refused");
    assert_eq!(err.error_code.as_deref(), Some("invalid_params"));
    assert_eq!(err.data["max_bytes"], 2048);

    // Exactly 2048 bytes is accepted.
    client
        .tools_call("host.feedback.send", json!({"kind": "bug", "text": "a".repeat(2048)}))
        .await
        .expect("2048 bytes is within the limit");
}

#[tokio::test]
async fn control_characters_are_stripped_before_storage() {
    let server = TestServer::start().await;
    let client = McpClient::new(&server.base_url);

    let res = client
        .tools_call(
            "host.feedback.send",
            json!({"kind": "docs_gap", "text": "ab\u{0}c\u{7}d\ne\tf"}),
        )
        .await
        .expect("a text with control characters is stored, not refused");
    let id = extract_structured(&res)["feedback_id"].as_str().unwrap().to_string();
    let row = server.state.db.get_feedback(id).await.unwrap().expect("row");
    assert_eq!(row.text, "abcd\ne\tf", "\\x00 and other control chars removed, \\n and \\t kept");
}
