//! AC8 (P0) — Given the admin `/healthz`, When read, Then it carries
//! `scheduler_last_tick_unix` within the last 60s and `schedules_enabled`.

mod common;
use common::{ADMIN_KEY, TestServer, extract_structured, signup_and_make_pro};
use serde_json::json;

#[tokio::test]
async fn healthz_reports_scheduler_liveness_and_schedules_enabled() {
    let server = TestServer::start().await;
    let (_ns, key, _tenant_id) = signup_and_make_pro(&server, "AC8 Tenant", "cus_ac8").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "pinger", "kind": "echo", "spec": {"schema": {"type": "object"}}}),
        )
        .await
        .expect("publish");
    extract_structured(
        &client
            .tools_call(
                "host.trigger.set",
                json!({"tool": "pinger", "schedule": "*/5 * * * *"}),
            )
            .await
            .expect("trigger.set"),
    );

    // `spawn_scheduler` ticks once immediately at startup, so
    // `scheduler_last_tick_unix` is already set without waiting out a real
    // 30s cadence.
    mcphost::triggers::tick_once(&server.state).await.expect("tick");

    let health: serde_json::Value = reqwest::Client::new()
        .get(format!("{}/healthz", server.base_url))
        .bearer_auth(ADMIN_KEY)
        .send()
        .await
        .expect("GET /healthz")
        .json()
        .await
        .expect("parse /healthz");

    let last_tick = health["scheduler_last_tick_unix"]
        .as_i64()
        .expect("scheduler_last_tick_unix must be a number");
    let now = mcphost::state::now_unix();
    assert!(
        (now - last_tick).abs() <= 60,
        "scheduler_last_tick_unix must be within the last 60s: {health:?}"
    );
    assert_eq!(health["schedules_enabled"], json!(1), "{health:?}");
}
