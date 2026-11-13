//! PRD-mcphost-tenant-templates
//! AC3 -- Given a free tenant already holding 3 event triggers, When
//! `webhook-inbox` is applied, Then the error is `plan_limit` with
//! `data.ceiling == "event_triggers_max"` and the tenant's tool, table and
//! trigger counts are unchanged.

use crate::ttpl;
use serde_json::json;

#[tokio::test]
async fn ttpl_ac03_webhook_inbox_over_event_triggers_max_creates_nothing() {
    let fx = ttpl::start().await;
    fx.fill_event_triggers(3).await;
    let before = fx.inventory().await;
    assert_eq!(before.1.len(), 3, "the tenant holds 3 event triggers: {before:?}");

    let err = fx.apply("webhook-inbox").await.expect_err("a fourth event-class trigger is over the free ceiling");
    assert_eq!(err.error_code.as_deref(), Some("plan_limit"), "{err:?}");
    assert_eq!(err.data["ceiling"], json!("event_triggers_max"), "{err:?}");

    assert_eq!(fx.inventory().await, before, "tool, table and trigger counts are unchanged");
}
