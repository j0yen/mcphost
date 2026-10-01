//! PRD-mcphost-one-next-tool AC9 (P1) — Given seven days in which 10 hints
//! were shown and 3 were followed, When `host.usage` is called, Then
//! `hints.shown_7d == 10` and `hints.followed_7d == 3`, independent of the
//! response's own `window`/`by` argument.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use mcphost::state::now_unix;
use serde_json::json;

#[tokio::test]
async fn host_usage_reports_hints_shown_and_followed_over_a_fixed_seven_days() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "AC9 Tenant").await;
    let tenant = server
        .state
        .db
        .find_tenant_by_namespace(ns.clone())
        .await
        .expect("find tenant")
        .expect("tenant exists");

    let now = now_unix();
    // 10 shown within 7 days, 3 of them followed.
    for i in 0..10 {
        server
            .state
            .db
            .insert_hint_event_for_test(tenant.id, "host.tool_call", now - i * 3600, i < 3)
            .await
            .expect("seed hint event");
    }
    // Outside the 7-day window entirely -- must not count toward either figure.
    server
        .state
        .db
        .insert_hint_event_for_test(tenant.id, "host.tool_call", now - 8 * 24 * 3600, true)
        .await
        .expect("seed stale hint event");

    let client = McpClient::with_bearer(&server.base_url, &key);
    let result = client.tools_call("host.usage", json!({})).await.expect("host.usage");
    let usage = extract_structured(&result);
    assert_eq!(usage["hints"]["shown_7d"], json!(10), "{usage:?}");
    assert_eq!(usage["hints"]["followed_7d"], json!(3), "{usage:?}");
}
