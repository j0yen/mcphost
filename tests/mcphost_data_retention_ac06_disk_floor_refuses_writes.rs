//! PRD-mcphost-data-retention
//! AC6 (P0) — Given free space below the floor (simulated by a test
//! hook), When `host.tool_call` runs, Then it returns
//! `service_unavailable: disk floor` and `healthz` reports
//! `disk_ok: false`.
//!
//! Free space is pinned via `DiskGuard::set_free_bytes_override_for_test`
//! rather than actually filling the test's tmpfs -- same "flip an
//! internal knob for a test" shape `Db::set_query_only` uses for AC14's
//! analogous unwritable-database simulation.

use crate::common;
use common::{ADMIN_KEY, McpClient, extract_structured, signup};
use serde_json::json;

async fn admin_healthz(base_url: &str) -> serde_json::Value {
    reqwest::Client::new()
        .get(format!("{base_url}/healthz"))
        .bearer_auth(ADMIN_KEY)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap()
}

#[tokio::test]
async fn below_floor_refuses_tool_call_and_healthz_reports_disk_not_ok() {
    let server = common::TestServer::start().await;
    let (_tenant_ns, key) = signup(&server.base_url, "AC6 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let publish = client
        .tools_call(
            "host.tool_publish",
            json!({"name": "hello", "kind": "echo", "spec": {"schema": {"type": "object"}}}),
        )
        .await
        .expect("publish while above the floor");
    let qualified = extract_structured(&publish)["name"]
        .as_str()
        .expect("name field")
        .to_string();
    let _ = qualified;

    let health = admin_healthz(&server.base_url).await;
    assert_eq!(health["disk_ok"], json!(true), "disk_ok must be true above the floor");

    server
        .state
        .disk_guard
        .set_free_bytes_override_for_test(Some(1));

    let err = client
        .tools_call("host.tool_call", json!({"name": "hello", "args": {}}))
        .await
        .expect_err("host.tool_call must refuse below the disk floor");
    assert_eq!(err.error_code.as_deref(), Some("service_unavailable"));
    assert!(
        err.message.contains("disk floor"),
        "message must mention 'disk floor', got {:?}",
        err.message
    );

    let health = admin_healthz(&server.base_url).await;
    assert_eq!(
        health["disk_ok"],
        json!(false),
        "healthz must report disk_ok: false below the floor"
    );

    server
        .state
        .disk_guard
        .set_free_bytes_override_for_test(Some(u64::MAX));
    let health = admin_healthz(&server.base_url).await;
    assert_eq!(health["disk_ok"], json!(true), "disk_ok must recover once above the floor again");
}
