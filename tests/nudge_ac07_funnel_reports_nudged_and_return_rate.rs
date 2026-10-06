//! PRD-mcphost-second-session-nudge
//! AC7 (P0) — Given five nudged tenants of which one later has
//! `second_session_unix > nudged_unix`, When `admin.funnel` runs, Then
//! its source row shows `nudged 5`, `returned_after_nudge 1`,
//! `return_rate 0.2`.

use crate::common;
use common::{ADMIN_KEY, McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn funnel_by_source_reports_nudged_returned_and_rate() {
    let server = TestServer::start().await;

    let mut tenant_ids = Vec::new();
    for i in 0..5 {
        let (namespace, _key) = signup(&server.base_url, &format!("AC7 Tenant {i}")).await;
        let tenant = server
            .state
            .db
            .find_tenant_by_namespace(namespace)
            .await
            .expect("find tenant")
            .expect("tenant exists");
        tenant_ids.push(tenant.id);
    }

    let nudged_unix = mcphost::state::now_unix() - 100;
    for &id in &tenant_ids {
        server
            .state
            .db
            .set_tenant_stamp_for_test(id, "nudged_unix", nudged_unix)
            .await
            .expect("set nudged_unix");
    }
    // Exactly one of the five returns after its nudge.
    server
        .state
        .db
        .set_tenant_stamp_for_test(tenant_ids[0], "second_session_unix", nudged_unix + 50)
        .await
        .expect("set second_session_unix");

    let admin = McpClient::with_bearer(&server.base_url, ADMIN_KEY);
    let report = extract_structured(
        &admin
            .tools_call("admin.funnel", json!({"days": 7}))
            .await
            .expect("admin.funnel"),
    );
    let loopback = &report["by_source"]["loopback"];
    assert_eq!(loopback["nudged"], json!(5), "{report}");
    assert_eq!(loopback["returned_after_nudge"], json!(1), "{report}");
    assert_eq!(loopback["return_rate"].as_f64(), Some(0.2), "{report}");
}
