//! PRD-mcphost-tenant-delete
//! AC1 — Given a tenant with 3 tools, 2 secrets, 20 calls, 10 log lines and
//! a registry document, When `admin.tenant_delete` runs with the admin
//! key, Then all of those rows are gone, the result reports the counts,
//! and `/healthz` `tenants_total` and `tools_total` drop accordingly.

mod common;
use common::{ADMIN_KEY, McpClient, TestServer, extract_structured, signup};
use serde_json::json;

// PRD-mcphost-healthz-minimal: `tenants_total`/`tools_total` moved behind
// the admin bearer -- the anonymous body is `{"ok": true/false}`.
async fn healthz(base_url: &str) -> serde_json::Value {
    reqwest::Client::new()
        .get(format!("{base_url}/healthz"))
        .bearer_auth(ADMIN_KEY)
        .send()
        .await
        .expect("GET /healthz")
        .json()
        .await
        .expect("parse /healthz")
}

#[tokio::test]
async fn deleting_a_tenant_cascades_every_owned_row() {
    let server = TestServer::start().await;
    let (tenant_ns, _key) = signup(&server.base_url, "Doomed Tenant").await;
    let tenant = server
        .state
        .db
        .find_tenant_by_namespace(tenant_ns.clone())
        .await
        .unwrap()
        .expect("tenant");

    for i in 0..3 {
        server
            .state
            .db
            .upsert_tool(
                tenant.id,
                format!("tool{i}"),
                "echo".to_string(),
                json!({"schema": {"type": "object"}}),
            )
            .await
            .expect("upsert_tool");
    }
    for i in 0..2 {
        let (ct, nonce) = server.state.secrets.encrypt("s3cr3t").unwrap();
        server
            .state
            .db
            .upsert_secret(tenant.id, format!("secret{i}"), ct, nonce)
            .await
            .expect("upsert_secret");
    }
    for _ in 0..20 {
        server
            .state
            .db
            .record_call(tenant.id, "tool0".to_string(), 5, true, None, None, None, "ok")
            .await
            .expect("record_call");
    }
    for i in 0..10 {
        server
            .state
            .db
            .append_log(tenant.id, "tool0".to_string(), format!("line {i}"))
            .await
            .expect("append_log");
    }
    server
        .state
        .db
        .upsert_registry_document(
            tenant.id,
            tenant_ns.clone(),
            json!({"name": tenant_ns, "endpoint": "http://example.invalid"}),
        )
        .await
        .expect("upsert_registry_document");

    let before = healthz(&server.base_url).await;

    let admin = McpClient::with_bearer(&server.base_url, ADMIN_KEY);
    let result = admin
        .tools_call("admin.tenant_delete", json!({"tenant": tenant_ns}))
        .await
        .expect("admin.tenant_delete");
    let structured = extract_structured(&result);
    assert_eq!(structured["tenant"], json!(tenant_ns));
    assert_eq!(structured["namespace"], json!(tenant_ns));
    assert_eq!(structured["tools_removed"], json!(3));
    assert_eq!(structured["secrets_removed"], json!(2));
    assert_eq!(structured["calls_removed"], json!(20));
    assert_eq!(structured["logs_removed"], json!(10));

    // Every owned row is actually gone.
    assert!(
        server
            .state
            .db
            .find_tenant_by_namespace(tenant_ns.clone())
            .await
            .unwrap()
            .is_none()
    );
    assert!(server.state.db.list_tools(tenant.id).await.unwrap().is_empty());
    assert!(
        server
            .state
            .db
            .list_secret_names(tenant.id)
            .await
            .unwrap()
            .is_empty()
    );
    let usage = server.state.db.usage(tenant.id, 24 * 3600).await.unwrap();
    assert_eq!(usage.calls, 0, "calls rows must be gone");
    assert!(
        server
            .state
            .db
            .tail_logs(tenant.id, "tool0".to_string(), 100)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        server
            .state
            .db
            .get_registry_document(tenant_ns.clone())
            .await
            .unwrap()
            .is_none()
    );

    let after = healthz(&server.base_url).await;
    assert_eq!(
        after["tenants_total"].as_i64().unwrap(),
        before["tenants_total"].as_i64().unwrap() - 1
    );
    assert_eq!(
        after["tools_total"].as_i64().unwrap(),
        before["tools_total"].as_i64().unwrap() - 3
    );
}
