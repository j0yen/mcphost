//! PRD-mcphost-scheduled-job-receipts AC7 (P1) — Given `limit: 51`, When
//! called, Then `args_invalid` names `limit ≤ 50`; Given a tool with no
//! triggers, Then `rows` is empty and `summary.runs == 0` without error.

use crate::common;

use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn limit_above_fifty_is_args_invalid_and_a_tool_without_triggers_is_empty() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "SJRC AC7 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "idle_tool", "kind": "echo", "spec": {"schema": {"type": "object"}}}),
        )
        .await
        .expect("publish");

    let err = client
        .tools_call("host.schedule_receipts", json!({"tool": "idle_tool", "limit": 51}))
        .await
        .expect_err("limit 51 must be refused");
    assert_eq!(err.error_code.as_deref(), Some("args_invalid"), "{err:?}");
    assert!(err.message.contains("maximum of 50"), "the error must name the ceiling: {err:?}");

    let ok = extract_structured(
        &client
            .tools_call("host.schedule_receipts", json!({"tool": "idle_tool", "limit": 50}))
            .await
            .expect("limit 50 is the ceiling, accepted"),
    );
    assert_eq!(ok["rows"], json!([]), "{ok:?}");
    assert_eq!(ok["summary"]["runs"], json!(0), "{ok:?}");
    assert_eq!(ok["summary"]["streak_failed"], json!(0), "{ok:?}");
}
