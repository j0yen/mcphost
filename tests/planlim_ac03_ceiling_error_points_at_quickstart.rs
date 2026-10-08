//! AC3 (PRD-mcphost-plan-limits-generated) — Given a call with
//! `budget.max_tool_latency_ms` above the ceiling, When rejected, Then the
//! error data carries `see: host.quickstart.limits.plan` alongside
//! `ceiling`, `requested`, `next`.

use crate::common;
use common::{McpClient, TestServer, signup};
use serde_json::json;

#[tokio::test]
async fn latency_ceiling_error_carries_see_pointer() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "PlanLim AC3").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "echoer", "kind": "echo", "spec": {"schema": {"type": "object"}}}),
        )
        .await
        .expect("publish ok");

    let err = client
        .tools_call(
            "host.tool_call",
            json!({"name": "echoer", "args": {}, "budget": {"max_tool_latency_ms": 180000}}),
        )
        .await
        .expect_err("above the free ceiling must be refused");
    assert_eq!(err.error_code.as_deref(), Some("budget_ceiling_exceeded"));
    let data = &err.data;
    assert_eq!(data["see"], json!("host.quickstart.limits.plan"));
    assert_eq!(data["ceiling"], json!(30000));
    assert_eq!(data["requested"], json!(180000));
    assert!(!data["next"].is_null(), "next must be present: {data}");
    assert_eq!(data["field"], json!("budget.max_tool_latency_ms"));
}
