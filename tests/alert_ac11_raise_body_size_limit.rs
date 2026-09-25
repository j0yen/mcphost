//! PRD-mcphost-alerting-webhook
//! AC11 — Given `admin.alerts.raise` is called with a body of 64 KiB, When
//! it runs, Then it is rejected with a size error at 16 KiB and stored at
//! 15 KiB.

use crate::common;
use common::{ADMIN_KEY, McpClient, TestServer, extract_structured};
use serde_json::{Value, json};

const MAX_ALERT_BODY_BYTES: usize = 16 * 1024;

/// A `body` object whose serialized JSON is at least `target_bytes` long
/// (via a single padding field of ASCII 'x', which serializes with no
/// escaping, so the string length maps directly to serialized bytes).
fn body_of_at_least(target_bytes: usize) -> Value {
    let overhead = serde_json::to_string(&json!({"padding": ""})).unwrap().len();
    let padding_len = target_bytes.saturating_sub(overhead);
    json!({"padding": "x".repeat(padding_len)})
}

#[tokio::test]
async fn oversized_body_rejected_undersized_body_stored() {
    let server = TestServer::start().await;
    let admin = McpClient::with_bearer(&server.base_url, ADMIN_KEY);

    let oversized = body_of_at_least(64 * 1024);
    assert!(serde_json::to_string(&oversized).unwrap().len() >= 64 * 1024);
    let err = admin
        .tools_call(
            "admin.alerts.raise",
            json!({"key": "test.ac11.oversized", "severity": "warn", "title": "AC11", "body": oversized}),
        )
        .await
        .expect_err("a 64 KiB body must be rejected");
    assert_eq!(err.error_code.as_deref(), Some("alert_body_too_large"));
    assert_eq!(err.data["limit_bytes"], MAX_ALERT_BODY_BYTES);
    assert!(
        server
            .state
            .db
            .most_recent_alert_for_key("test.ac11.oversized".to_string())
            .await
            .expect("query")
            .is_none(),
        "a rejected raise must not store a row"
    );

    let undersized = body_of_at_least(15 * 1024);
    let undersized_bytes = serde_json::to_string(&undersized).unwrap().len();
    assert!(undersized_bytes < MAX_ALERT_BODY_BYTES, "{undersized_bytes}");
    let result = extract_structured(
        &admin
            .tools_call(
                "admin.alerts.raise",
                json!({"key": "test.ac11.undersized", "severity": "warn", "title": "AC11", "body": undersized}),
            )
            .await
            .expect("a 15 KiB body must be accepted"),
    );
    let id = result["id"].as_i64().expect("id");
    let row = server
        .state
        .db
        .get_alert_for_test(id)
        .await
        .expect("query")
        .expect("row stored");
    assert_eq!(row.key, "test.ac11.undersized");
}
