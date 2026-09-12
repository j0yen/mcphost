//! AC4 (P0) — Given a free tenant, When `schedule="* * * * *"` is set,
//! Then `trigger_interval_too_short` names 300 s.

use crate::common;
use common::{TestServer, signup};
use serde_json::json;

#[tokio::test]
async fn every_minute_on_free_plan_is_interval_too_short() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "AC4 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "pinger", "kind": "echo", "spec": {"schema": {"type": "object"}}}),
        )
        .await
        .expect("publish");

    let err = client
        .tools_call(
            "host.trigger.set",
            json!({"tool": "pinger", "schedule": "* * * * *"}),
        )
        .await
        .expect_err("every-minute on free plan must be refused");
    assert_eq!(err.error_code.as_deref(), Some("trigger_interval_too_short"));
    assert_eq!(err.data["schedule_min_interval_s"], json!(300), "{:?}", err.data);
}
