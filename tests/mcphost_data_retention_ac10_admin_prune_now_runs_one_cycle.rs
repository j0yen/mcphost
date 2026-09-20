//! PRD-mcphost-data-retention
//! AC10 (P2) — Given admin scope, When `admin.prune_now` is called, Then
//! one cycle runs and returns its counts.

use crate::common;
use common::{ADMIN_KEY, McpClient, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn admin_prune_now_runs_one_cycle_and_returns_deleted_counts() {
    let server = common::TestServer::start().await;
    let (_namespace, key) = signup(&server.base_url, "AC10 Tenant").await;
    let tenant = server
        .state
        .db
        .find_tenant_by_namespace(_namespace.clone())
        .await
        .expect("query tenant")
        .expect("tenant exists");

    server
        .state
        .db
        .set_retention_days_for_test("calls".to_string(), 90)
        .await
        .expect("set calls retention window");

    let now = mcphost::state::now_unix();
    let ninety_one_days_ago = now - 91 * 86_400;
    for _ in 0..3 {
        server
            .state
            .db
            .insert_calls_row_for_test(tenant.id, ninety_one_days_ago)
            .await
            .expect("insert expired call");
    }

    // A non-admin (tenant-scoped) caller may not run the admin tool.
    let tenant_client = McpClient::with_bearer(&server.base_url, &key);
    let forbidden = tenant_client
        .tools_call("admin.prune_now", json!({}))
        .await
        .expect_err("a tenant key must not reach admin.prune_now");
    assert_eq!(forbidden.error_code.as_deref(), Some("forbidden"));

    let admin = McpClient::with_bearer(&server.base_url, ADMIN_KEY);
    let result = admin
        .tools_call("admin.prune_now", json!({}))
        .await
        .expect("admin.prune_now");
    let body = extract_structured(&result);

    assert_eq!(
        body["deleted"]["calls"].as_i64(),
        Some(3),
        "admin.prune_now must return the counts of the cycle it just ran, got {body:?}"
    );
    let started = body["started_unix"].as_i64().expect("started_unix");
    let finished = body["finished_unix"].as_i64().expect("finished_unix");
    assert!(finished >= started, "finished_unix must not precede started_unix");
}
