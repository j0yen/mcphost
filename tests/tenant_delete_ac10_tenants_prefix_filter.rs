//! PRD-mcphost-tenant-delete
//! AC10 (P1) — Given `admin.tenants("panel_")`, When called, Then only
//! tenants with that prefix are listed.

use crate::common;
use common::{ADMIN_KEY, McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn tenants_prefix_filters_the_listing() {
    let server = TestServer::start().await;
    let (ns_a, _) = signup(&server.base_url, "panel_a").await;
    let (ns_b, _) = signup(&server.base_url, "panel_b").await;
    let (ns_real, _) = signup(&server.base_url, "real-user").await;

    let admin = McpClient::with_bearer(&server.base_url, ADMIN_KEY);

    // No prefix: everything is listed, same as before this PRD.
    let all = admin
        .tools_call("admin.tenants", json!({}))
        .await
        .expect("admin.tenants (no prefix)");
    let all = extract_structured(&all);
    let all_tenants: Vec<&str> = all["tenants"]
        .as_array()
        .expect("tenants array")
        .iter()
        .map(|t| t["tenant"].as_str().unwrap())
        .collect();
    assert!(all_tenants.contains(&ns_a.as_str()));
    assert!(all_tenants.contains(&ns_b.as_str()));
    assert!(all_tenants.contains(&ns_real.as_str()));

    // With a prefix: only the matching tenants are listed, no delete
    // counts attached (that's the batch-delete dry run's shape, not this
    // one's).
    let filtered = admin
        .tools_call("admin.tenants", json!({"prefix": "panel_"}))
        .await
        .expect("admin.tenants (prefix)");
    let filtered = extract_structured(&filtered);
    let filtered_tenants: Vec<&str> = filtered["tenants"]
        .as_array()
        .expect("tenants array")
        .iter()
        .map(|t| t["tenant"].as_str().unwrap())
        .collect();
    assert_eq!(filtered_tenants.len(), 2, "only panel_a and panel_b must match");
    assert!(filtered_tenants.contains(&ns_a.as_str()));
    assert!(filtered_tenants.contains(&ns_b.as_str()));
    assert!(!filtered_tenants.contains(&ns_real.as_str()));
}
