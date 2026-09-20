//! PRD-mcphost-data-retention
//! AC3 (P0) — Given a table not in the policy, When the prune runs, Then
//! its row count is unchanged.
//!
//! `tenants` is never listed in `retention::POLICY_TABLES` (requirement 1:
//! "a table not listed is never pruned") -- it holds every signed-up
//! tenant, which the prune must never touch regardless of age.

use crate::common;

#[tokio::test]
async fn tenants_row_count_is_unchanged_by_a_prune() {
    let server = common::TestServer::start().await;
    common::signup(&server.base_url, "AC3 Tenant One").await;
    common::signup(&server.base_url, "AC3 Tenant Two").await;

    let before = server
        .state
        .db
        .table_row_count_for_test("tenants".to_string())
        .await
        .expect("count tenants before");
    assert_eq!(before, 2);

    server.state.db.prune_once().await.expect("prune_once");

    let after = server
        .state
        .db
        .table_row_count_for_test("tenants".to_string())
        .await
        .expect("count tenants after");
    assert_eq!(after, before, "a table absent from the retention policy must be untouched");
}
