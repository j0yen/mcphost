//! PRD-mcphost-tenant-delete
//! AC4 — Given the same three tenants, When the call runs with
//! `dry_run=false`, Then `panel_a` and `panel_b` and all their rows are
//! gone and `real-user` is intact.

use crate::common;
use common::{ADMIN_KEY, McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn dry_run_false_deletes_matches_and_spares_the_rest() {
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
        .tools_call(
            "admin.tenant_delete_by_prefix",
            json!({"prefix": "panel_", "dry_run": false}),
        )
        .await
        .expect("admin.tenant_delete_by_prefix (dry_run=false)");
    let structured = extract_structured(&result);
    assert_eq!(structured["dry_run"], json!(false));
    let deleted = structured["deleted"].as_array().expect("deleted array");
    assert_eq!(deleted.len(), 2);
    assert_eq!(structured["tools_removed"], json!(1));

    assert!(
        server
            .state
            .db
            .find_tenant_by_namespace(ns_a)
            .await
            .unwrap()
            .is_none(),
        "panel_a must be gone"
    );
    assert!(
        server
            .state
            .db
            .find_tenant_by_namespace(ns_b)
            .await
            .unwrap()
            .is_none(),
        "panel_b must be gone"
    );
    assert!(
        server
            .state
            .db
            .find_tenant_by_namespace(ns_real.clone())
            .await
            .unwrap()
            .is_some(),
        "real-user must be intact"
    );
    assert!(
        server
            .state
            .db
            .list_tools(tenant_a.id)
            .await
            .unwrap()
            .is_empty(),
        "panel_a's tool row must be gone too"
    );
}
