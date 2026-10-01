//! PRD-mcphost-data-retention
//! AC6 (P0) — Given free space below the floor (simulated by a test
//! hook), When `host.tool_call` runs, Then it returns `service_unavailable`
//! (`data.reason: "disk_floor"`) and `healthz` reports `disk_ok: false`.
//! The client message itself was `"disk floor: <N> bytes free is below the
//! <M>-byte floor"` until PRD-mcphost-first-hour-support-surface
//! requirement 1 (AC3) made it the fixed generic string every 5xx this
//! host can return now uses -- host capacity numbers never belonged on the
//! wire; `data.reason` still names it.
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
    // PRD-mcphost-first-hour-support-surface requirement 1 / AC3: the
    // client-facing message is now the fixed generic string (no byte
    // counts, no host numbers) -- this AC's own "disk floor" wording moved
    // to `data.reason`, which was already there and is unchanged; the
    // 5xx-message test (`support_ac03_*`) covers the generic-message
    // contract itself.
    assert!(
        err.message.starts_with("service unavailable; request_id="),
        "message must be the fixed generic string, got {:?}",
        err.message
    );
    assert_eq!(err.data["reason"], json!("disk_floor"), "{:?}", err.data);

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
