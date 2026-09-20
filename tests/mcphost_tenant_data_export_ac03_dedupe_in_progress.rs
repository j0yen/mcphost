//! PRD-mcphost-tenant-data-export
//! AC3 (P0) -- Given an export in progress, When `host.export` is called
//! again, Then the same run id is returned.

use serde_json::json;

use crate::common;
use common::{McpClient, extract_structured, signup};

#[tokio::test]
async fn second_export_call_while_running_returns_same_run_id() {
    let server = common::TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "Export AC3 Tenant").await;
    let tenant = server
        .state
        .db
        .find_tenant_by_namespace(ns)
        .await
        .expect("find tenant")
        .expect("tenant exists");

    // Simulate an export already in progress via the same atomic
    // check-or-insert `host.export` itself uses -- deterministic, unlike
    // racing two real `host.export` calls against wall-clock timing (this
    // tenant has no tools/state, so a real export can finish well before a
    // second real call reaches the server).
    let existing_run_id = "01TESTRUNIDFORAC3DEDUPE00".to_string();
    server
        .state
        .db
        .start_export_run(
            tenant.id,
            existing_run_id.clone(),
            mcphost::export::EXPORT_TOOL_NAME.to_string(),
            300,
            "{}".to_string(),
        )
        .await
        .expect("start_export_run ok");

    let client = McpClient::with_bearer(&server.base_url, &key);
    let result = extract_structured(
        &client
            .tools_call("host.export", json!({}))
            .await
            .expect("host.export ok"),
    );
    assert_eq!(
        result["run_id"],
        json!(existing_run_id),
        "a host.export call while one is already running must return that run's id"
    );
    assert_eq!(result["status"], json!("running"));
}
