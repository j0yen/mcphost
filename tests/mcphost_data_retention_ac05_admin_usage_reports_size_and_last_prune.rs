//! PRD-mcphost-data-retention
//! AC5 (P0) — Given a completed prune, When `admin.usage` is read, Then
//! `rows_by_table`, `db_bytes`, and `last_prune.deleted` are present and
//! consistent with the deletion.

use crate::common;
use common::{ADMIN_KEY, McpClient, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn admin_usage_reports_db_size_and_last_prune_deleted_counts() {
    let server = common::TestServer::start().await;
    let (namespace, _key) = signup(&server.base_url, "AC5 Tenant").await;
    let tenant = server
        .state
        .db
        .find_tenant_by_namespace(namespace)
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
    for _ in 0..5 {
        server
            .state
            .db
            .insert_calls_row_for_test(tenant.id, ninety_one_days_ago)
            .await
            .expect("insert expired call");
    }

    server.state.db.prune_once().await.expect("prune_once");

    let remaining_calls = server
        .state
        .db
        .table_row_count_for_test("calls".to_string())
        .await
        .expect("count calls after prune");

    let admin = McpClient::with_bearer(&server.base_url, ADMIN_KEY);
    let usage = admin
        .tools_call("admin.usage", json!({}))
        .await
        .expect("admin.usage");
    let body = extract_structured(&usage);

    assert!(
        body["db_bytes"].as_i64().unwrap_or(0) > 0,
        "db_bytes must be a positive byte count, got {:?}",
        body["db_bytes"]
    );
    assert!(
        body.get("db_page_free_bytes").is_some(),
        "db_page_free_bytes must be present"
    );
    assert_eq!(
        body["rows_by_table"]["calls"].as_i64(),
        Some(remaining_calls),
        "rows_by_table.calls must match the real post-prune row count"
    );
    assert_eq!(
        body["last_prune"]["ok"].as_bool(),
        Some(true),
        "last_prune.ok must be true after a clean cycle"
    );
    assert_eq!(
        body["last_prune"]["deleted"]["calls"].as_i64(),
        Some(5),
        "last_prune.deleted.calls must equal the 5 rows this prune actually deleted"
    );
}
