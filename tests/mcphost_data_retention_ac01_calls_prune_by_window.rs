//! PRD-mcphost-data-retention
//! AC1 (P0) — Given `calls` rows aged 91 and 89 days and
//! `MCPHOST_RETENTION_CALLS_DAYS=90`, When the prune runs, Then the
//! 91-day row is deleted and the 89-day row remains.
//!
//! The 90-day window is set via `Db::set_retention_days_for_test` rather
//! than the real `$MCPHOST_RETENTION_CALLS_DAYS` process env var --
//! mutating process environment from one of several test functions
//! sharing a test binary would be flaky (same rationale
//! `TestServer::start_with_signup_rate_limit`'s own doc comment gives for
//! the analogous `$MCPHOST_SIGNUP_RATE_LIMIT_PER_HOUR` case). 90 is also
//! this table's documented default, so the override is really just making
//! the test's assumption explicit rather than changing behavior.

use crate::common;

#[tokio::test]
async fn ninety_one_day_row_pruned_eighty_nine_day_row_kept() {
    let server = common::TestServer::start().await;
    let (namespace, _key) = common::signup(&server.base_url, "AC1 Tenant").await;
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
    let eighty_nine_days_ago = now - 89 * 86_400;
    server
        .state
        .db
        .insert_calls_row_for_test(tenant.id, ninety_one_days_ago)
        .await
        .expect("insert 91-day-old call");
    server
        .state
        .db
        .insert_calls_row_for_test(tenant.id, eighty_nine_days_ago)
        .await
        .expect("insert 89-day-old call");

    let report = server.state.db.prune_once().await.expect("prune_once");
    assert_eq!(
        report.deleted.get("calls").copied(),
        Some(1),
        "exactly the 91-day-old row should be deleted, deleted={:?}",
        report.deleted
    );

    let remaining = server
        .state
        .db
        .table_row_count_for_test("calls".to_string())
        .await
        .expect("count calls");
    assert_eq!(remaining, 1, "the 89-day-old row must survive the prune");
}
