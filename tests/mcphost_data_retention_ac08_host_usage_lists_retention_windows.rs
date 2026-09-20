//! PRD-mcphost-data-retention
//! AC8 (P1) — Given a tenant, When `host.usage` is called, Then the
//! retention windows are listed.

use crate::common;
use common::{McpClient, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn host_usage_lists_every_configured_retention_window() {
    let server = common::TestServer::start().await;
    let (_namespace, key) = signup(&server.base_url, "AC8 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let usage = client
        .tools_call("host.usage", json!({}))
        .await
        .expect("host.usage");
    let body = extract_structured(&usage);
    let retention_days = body["retention_days"]
        .as_object()
        .expect("retention_days must be an object");

    for table in [
        "calls",
        "calls_resource_usage",
        "events",
        "signup_events",
        "metering",
        "runs",
        "threads",
        "messages",
    ] {
        assert!(
            retention_days.get(table).and_then(serde_json::Value::as_i64).unwrap_or(0) > 0,
            "retention_days must list a positive window for '{table}', got {:?}",
            retention_days.get(table)
        );
    }
}
