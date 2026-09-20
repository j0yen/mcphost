//! PRD-mcphost-data-retention
//! AC2 (P0) — Given metering rows 399 days old, When the prune runs, Then
//! none is deleted.

use crate::common;

#[tokio::test]
async fn three_ninety_nine_day_old_metering_rows_survive_the_prune() {
    let server = common::TestServer::start().await;
    let (namespace, _key) = common::signup(&server.base_url, "AC2 Tenant").await;
    let tenant = server
        .state
        .db
        .find_tenant_by_namespace(namespace)
        .await
        .expect("query tenant")
        .expect("tenant exists");

    // The default window (400 days), made explicit the same way AC1 does.
    server
        .state
        .db
        .set_retention_days_for_test("metering".to_string(), 400)
        .await
        .expect("set metering retention window");

    let now = mcphost::state::now_unix();
    let three_ninety_nine_days_ago = now - 399 * 86_400;
    for _ in 0..3 {
        server
            .state
            .db
            .insert_meter_event_row_for_test(tenant.id, three_ninety_nine_days_ago)
            .await
            .expect("insert 399-day-old meter event");
    }

    let before = server
        .state
        .db
        .table_row_count_for_test("meter_events".to_string())
        .await
        .expect("count meter_events before");
    assert_eq!(before, 3);

    let report = server.state.db.prune_once().await.expect("prune_once");
    assert_eq!(
        report.deleted.get("meter_events").copied().unwrap_or(0),
        0,
        "no metering row under the 400-day window should be deleted, deleted={:?}",
        report.deleted
    );

    let after = server
        .state
        .db
        .table_row_count_for_test("meter_events".to_string())
        .await
        .expect("count meter_events after");
    assert_eq!(after, 3, "every 399-day-old metering row must survive the prune");
}
