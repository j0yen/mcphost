//! PRD-mcphost-alerting-webhook
//! AC3 — Given a tenant whose `calls_per_day` trips 20 times within 5 min,
//! When the 20th trip occurs, Then one `quota.trip` alert exists for that
//! tenant and plan knob; the 21st trip raises nothing new.

use crate::common;
use common::{McpClient, TestServer, signup};
use serde_json::json;

fn echo_spec() -> serde_json::Value {
    json!({"schema": {"type": "object"}})
}

#[tokio::test]
async fn twenty_refused_calls_raise_one_quota_trip_alert() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "Heavy User").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "echoer", "kind": "echo", "spec": echo_spec()}),
        )
        .await
        .expect("publish must succeed");

    let tenant = server
        .state
        .db
        .find_tenant_by_namespace(ns.clone())
        .await
        .expect("db query")
        .expect("tenant must exist");

    // Pre-seed the free plan's 500 ok calls today, same "seed directly
    // through the db handle" rationale `billing_ac03`'s own test already
    // uses -- every subsequent call this test makes is refused with
    // `quota_exceeded` from the very first one.
    for _ in 0..500 {
        server
            .state
            .db
            .record_call(tenant.id, "echoer".to_string(), 1, true, None, None, None, "ok", "external".to_string(), None)
            .await
            .expect("seed call");
    }

    let qualified = format!("{ns}.echoer");
    let quota_trip_key = format!("quota.trip:{ns}:calls_per_day");

    // Trips 1-19: below MCPHOST_ALERT_QUOTA_TRIP_THRESHOLD's default (20) --
    // no alert yet.
    for _ in 0..19 {
        client
            .tools_call(&qualified, json!({}))
            .await
            .expect_err("over-quota call must be refused");
    }
    assert!(
        server
            .state
            .db
            .most_recent_alert_for_key(quota_trip_key.clone())
            .await
            .expect("query")
            .is_none(),
        "no quota.trip alert before the 20th trip"
    );

    // Trip 20: raises the one alert.
    client
        .tools_call(&qualified, json!({}))
        .await
        .expect_err("over-quota call must be refused");
    let alert = server
        .state
        .db
        .most_recent_alert_for_key(quota_trip_key.clone())
        .await
        .expect("query")
        .expect("a quota.trip alert exists after the 20th trip");
    assert_eq!(alert.key, quota_trip_key);

    // Trip 21: raises nothing new -- same open row, not a second one.
    client
        .tools_call(&qualified, json!({}))
        .await
        .expect_err("over-quota call must be refused");
    let after = server
        .state
        .db
        .most_recent_alert_for_key(quota_trip_key)
        .await
        .expect("query")
        .expect("row still exists");
    assert_eq!(after.id, alert.id, "the 21st trip must not raise a second row");
}
