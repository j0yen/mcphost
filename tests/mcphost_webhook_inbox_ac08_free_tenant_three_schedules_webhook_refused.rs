//! PRD-mcphost-webhook-inbox
//! AC8 (P0) — Given a free tenant with 3 schedule triggers, When it sets a
//! webhook, Then it is refused with the schedules quota error.

use crate::common;
use common::{TestServer, signup};
use serde_json::json;

#[tokio::test]
async fn webhook_on_a_free_tenant_already_at_three_schedules_is_quota_exceeded() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "AC8 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "pinger", "kind": "echo", "spec": {"schema": {"type": "object"}}}),
        )
        .await
        .expect("publish");
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "on_payment", "kind": "echo", "spec": {"schema": {"type": "object"}}}),
        )
        .await
        .expect("publish");

    // Three distinct daily schedules -- webhooks share this same
    // schedules_max quota (requirement 5).
    for hour in 0..3 {
        client
            .tools_call(
                "host.trigger.set",
                json!({"tool": "pinger", "schedule": format!("0 {hour} * * *")}),
            )
            .await
            .unwrap_or_else(|e| panic!("schedule {hour} should be accepted: {} {}", e.code, e.message));
    }

    let err = client
        .tools_call(
            "host.trigger.set",
            json!({"tool": "on_payment", "kind": "webhook", "name": "pay"}),
        )
        .await
        .expect_err("a webhook on a free tenant already at 3 schedules must be refused");
    assert_eq!(err.error_code.as_deref(), Some("trigger_quota_exceeded"));
    assert_eq!(err.data["schedules_max"], json!(3), "{:?}", err.data);
}
