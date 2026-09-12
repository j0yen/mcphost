//! PRD-mcphost-tenant-delete
//! AC3 — Given three tenants named `panel_a`, `panel_b`, `real-user`, When
//! `admin.tenant_delete_by_prefix("panel_")` runs with the default dry
//! run, Then it lists `panel_a` and `panel_b` with counts, deletes
//! nothing, and `real-user` is not listed.

use crate::common;
use common::{ADMIN_KEY, McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn dry_run_lists_matches_with_counts_and_changes_nothing() {
    let server = TestServer::start().await;
    let (ns_a, _) = signup(&server.base_url, "panel_a").await;
    let (ns_b, _) = signup(&server.base_url, "panel_b").await;
    let (ns_real, _) = signup(&server.base_url, "real-user").await;

    let tenant_a = server
        .state
        .db
        .find_tenant_by_namespace(ns_a.clone())
        .await
        .unwrap()
        .expect("tenant a");
    server
        .state
        .db
        .upsert_tool(
            tenant_a.id,
            "t1".to_string(),
            "echo".to_string(),
            json!({"schema": {"type": "object"}}),
        )
        .await
        .expect("upsert_tool");

    let admin = McpClient::with_bearer(&server.base_url, ADMIN_KEY);
    let result = admin
        .tools_call("admin.tenant_delete_by_prefix", json!({"prefix": "panel_"}))
        .await
        .expect("admin.tenant_delete_by_prefix (default dry run)");
    let structured = extract_structured(&result);
    assert_eq!(structured["dry_run"], json!(true));
    let matched = structured["matched"].as_array().expect("matched array");
    assert_eq!(matched.len(), 2, "only panel_a and panel_b must match");
    let matched_tenants: Vec<&str> = matched
        .iter()
        .map(|m| m["tenant"].as_str().unwrap())
        .collect();
    assert!(matched_tenants.contains(&ns_a.as_str()));
    assert!(matched_tenants.contains(&ns_b.as_str()));
    assert!(!matched_tenants.contains(&ns_real.as_str()));

    let a_entry = matched
        .iter()
        .find(|m| m["tenant"] == json!(ns_a))
        .expect("panel_a entry");
    assert_eq!(a_entry["tools_removed"], json!(1));

    // Nothing was actually deleted.
    for ns in [&ns_a, &ns_b, &ns_real] {
        assert!(
            server
                .state
                .db
                .find_tenant_by_namespace(ns.clone())
                .await
                .unwrap()
                .is_some(),
            "{ns} must still exist after a dry run"
        );
    }
}
