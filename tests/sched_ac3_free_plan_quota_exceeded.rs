//! AC3 (P0) — Given a free tenant with three schedules, When a fourth is
//! set, Then `trigger_quota_exceeded` names `schedules_max: 3`.

mod common;
use common::{TestServer, signup};
use serde_json::json;

#[tokio::test]
async fn fourth_schedule_on_free_plan_is_quota_exceeded() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "AC3 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "pinger", "kind": "echo", "spec": {"schema": {"type": "object"}}}),
        )
        .await
        .expect("publish");

    // Three distinct daily schedules (well over free's 300s minimum
    // interval), each its own config_hash so none collides with another.
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
            json!({"tool": "pinger", "schedule": "0 3 * * *"}),
        )
        .await
        .expect_err("fourth schedule on free plan must be refused");
    assert_eq!(err.error_code.as_deref(), Some("trigger_quota_exceeded"));
    assert_eq!(err.data["schedules_max"], json!(3), "{:?}", err.data);
}
