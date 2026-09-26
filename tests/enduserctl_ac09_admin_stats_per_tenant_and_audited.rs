//! PRD-mcphost-end-user-audit-and-revoke
//! AC9 (P1) -- Given two tenants with end users, When
//! `admin.enduser.stats` runs, Then per-tenant active/revoked/purged
//! counts return and `admin_audit` records the call.

use crate::common;
use common::{ADMIN_KEY, McpClient, TestServer, extract_structured, signup};
use serde_json::json;

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

#[tokio::test]
async fn admin_stats_reports_per_tenant_counts_and_is_audited() {
    let server = TestServer::start().await;
    let (ns1, _key1) = signup(&server.base_url, "AC9 Tenant One").await;
    let (ns2, _key2) = signup(&server.base_url, "AC9 Tenant Two").await;
    let admin = McpClient::with_bearer(&server.base_url, ADMIN_KEY);

    let tenant1 = server.state.db.find_tenant_by_namespace(ns1.clone()).await.unwrap().expect("tenant1");
    let tenant2 = server.state.db.find_tenant_by_namespace(ns2.clone()).await.unwrap().expect("tenant2");
    let now = now_unix();

    // tenant1: 2 active, 1 revoked, 1 purged.
    server.state.db.insert_end_user_for_test(tenant1.id, "u1".to_string(), now, None, None).await.unwrap();
    server.state.db.insert_end_user_for_test(tenant1.id, "u2".to_string(), now, None, None).await.unwrap();
    server
        .state
        .db
        .insert_end_user_for_test(tenant1.id, "u3".to_string(), now, Some(now), None)
        .await
        .unwrap();
    server
        .state
        .db
        .insert_end_user_for_test(tenant1.id, "u4".to_string(), now, Some(now - 100), Some(now))
        .await
        .unwrap();

    // tenant2: 1 active only.
    server.state.db.insert_end_user_for_test(tenant2.id, "v1".to_string(), now, None, None).await.unwrap();

    let stats = extract_structured(
        &admin.tools_call("admin.enduser.stats", json!({})).await.expect("admin.enduser.stats ok"),
    );
    let tenants = stats["tenants"].as_array().expect("tenants array");

    let t1 = tenants.iter().find(|t| t["tenant"] == json!(ns1)).expect("tenant1 row present");
    assert_eq!(t1["active_30d"], json!(2), "{t1:?}");
    assert_eq!(t1["revoked"], json!(1), "{t1:?}");
    assert_eq!(t1["purged"], json!(1), "{t1:?}");

    let t2 = tenants.iter().find(|t| t["tenant"] == json!(ns2)).expect("tenant2 row present");
    assert_eq!(t2["active_30d"], json!(1), "{t2:?}");
    assert_eq!(t2["revoked"], json!(0), "{t2:?}");
    assert_eq!(t2["purged"], json!(0), "{t2:?}");

    let audit_rows = server.state.db.list_admin_audit(20, None).await.expect("list_admin_audit");
    assert!(
        audit_rows.iter().any(|r| r.action == "enduser_stats"),
        "admin_audit must record the admin.enduser.stats call: {audit_rows:?}"
    );
}
