//! Requirement 4 / 5 (not independently numbered ACs, but explicit in the
//! PRD's requirements list): `host.quickstart`'s `limits` names both
//! `schedules_max` and `schedule_min_interval_s`, and `host.usage` counts
//! scheduled runs under a `scheduled` block.

mod common;
use common::{TestServer, extract_structured, signup_and_make_pro};
use serde_json::json;
use std::time::{Duration, Instant};

#[tokio::test]
async fn quickstart_limits_and_usage_name_schedules() {
    let server = TestServer::start().await;
    let (_ns, key, _tenant_id) = signup_and_make_pro(&server, "Sched Usage Tenant", "cus_sched_usage").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    let quickstart = extract_structured(
        &client
            .tools_call("host.quickstart", json!({"kind": "echo"}))
            .await
            .expect("quickstart"),
    );
    assert_eq!(
        quickstart["limits"]["plan"]["schedules_max"],
        json!(25),
        "{quickstart:?}"
    );
    assert_eq!(
        quickstart["limits"]["plan"]["schedule_min_interval_s"],
        json!(60),
        "{quickstart:?}"
    );

    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "pinger", "kind": "echo", "spec": {"schema": {"type": "object"}}}),
        )
        .await
        .expect("publish");
    let set = extract_structured(
        &client
            .tools_call(
                "host.trigger.set",
                json!({"tool": "pinger", "schedule": "* * * * *"}),
            )
            .await
            .expect("trigger.set"),
    );
    let trigger_id = set["id"].as_str().expect("id").to_string();

    let now = mcphost::state::now_unix();
    server
        .state
        .db
        .update_trigger_after_fire(trigger_id.clone(), Some(now), None, 0)
        .await
        .expect("force due");
    mcphost::triggers::tick_once(&server.state).await.expect("tick");

    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let usage = extract_structured(
            &client
                .tools_call("host.usage", json!({}))
                .await
                .expect("usage"),
        );
        let done = usage["scheduled"]["done"].as_i64().unwrap_or(0);
        if done >= 1 {
            return;
        }
        if Instant::now() >= deadline {
            panic!("host.usage never counted the scheduled run as done: {usage:?}");
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}
