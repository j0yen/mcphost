//! PRD-mcphost-call-audit-export
//! AC4 (P0) — Given a Free tenant with rows 8 days old, When the prune job
//! runs, Then those rows are gone, `host.quickstart`
//! `limits.plan.calls_retention_days == 7`, and `host.calls.list {since:
//! "-30d"}` returns `data.truncated_before`.

use crate::common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn cae_ac04_free_tenant_rows_past_seven_days_are_pruned_and_the_list_names_the_cutoff() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "Retention Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    let tenant = server.state.db.find_tenant_by_namespace(ns).await.expect("find").expect("tenant");
    assert_eq!(tenant.plan, "free");
    let now = mcphost::state::now_unix();
    for _ in 0..3 {
        server.state.db.insert_calls_row_for_test(tenant.id, now - 8 * 86_400).await.expect("old row");
    }
    server.state.db.insert_calls_row_for_test(tenant.id, now - 6 * 86_400).await.expect("kept row");
    server.state.db.insert_calls_row_for_test(tenant.id, now - 60).await.expect("recent row");
    assert_eq!(server.state.db.calls_row_count_for_test(tenant.id).await.expect("count"), 5);

    server.state.db.prune_once().await.expect("prune_once");

    assert_eq!(
        server.state.db.calls_row_count_for_test(tenant.id).await.expect("count"),
        2,
        "the three 8-day-old rows are gone; the 6-day-old and the fresh row stay"
    );

    let quickstart = extract_structured(&client.tools_call("host.quickstart", json!({"kind": "echo"})).await.expect("quickstart"));
    assert_eq!(quickstart["limits"]["plan"]["calls_retention_days"], json!(7), "{quickstart}");
    let pro = mcphost::plans::PlanCatalog::default_catalog();
    assert_eq!(pro.get("pro").expect("pro").calls_retention_days, 90);

    let listing = extract_structured(
        &client.tools_call("host.calls.list", json!({"since": "-30d"})).await.expect("list -30d"),
    );
    let cutoff = listing["truncated_before"].as_str().unwrap_or_else(|| panic!("truncated_before: {listing}"));
    assert!(cutoff.starts_with("20"), "an RFC 3339 instant: {cutoff}");
    assert_eq!(listing["calls"].as_array().expect("calls").len(), 2, "{listing}");

    // A window inside the retention carries no truncation note.
    let inside = extract_structured(
        &client.tools_call("host.calls.list", json!({"since": "-24h"})).await.expect("list -24h"),
    );
    assert!(inside.get("truncated_before").is_none(), "{inside}");
}
