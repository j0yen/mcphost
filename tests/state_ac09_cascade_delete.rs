//! PRD-mcphost-tenant-state
//! AC9 — Given `admin.tenant_delete`, When it removes a tenant with state,
//! Then `tenant_state_kv` and its table rows for that tenant are gone in
//! the same transaction. Migration 0011 gives every new table `ON DELETE
//! CASCADE` to `tenants(id)`, the same mechanism PRD-mcphost-tenant-delete's
//! migration 0005 already relies on for `tools`/`secrets`/etc -- this test
//! is that same shape, applied to the new tables.

use crate::common;
use common::{ADMIN_KEY, McpClient, TestServer, signup};
use serde_json::json;

#[tokio::test]
async fn deleting_a_tenant_cascades_its_state_kv_and_table_rows() {
    let server = TestServer::start().await;
    let (tenant_ns, key) = signup(&server.base_url, "Stateful Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call("host.state.set", json!({"key": "a", "value": 1}))
        .await
        .expect("set a");
    client
        .tools_call(
            "host.state.table_create",
            json!({"name": "notes", "schema": {"body": "text"}}),
        )
        .await
        .expect("table_create");
    client
        .tools_call(
            "host.state.insert",
            json!({"table": "notes", "rows": [{"body": "one"}, {"body": "two"}]}),
        )
        .await
        .expect("insert");

    let tenant = server
        .state
        .db
        .find_tenant_by_namespace(tenant_ns.clone())
        .await
        .unwrap()
        .expect("tenant");
    assert!(
        server
            .state
            .db
            .state_kv_get(tenant.id, "a".to_string())
            .await
            .unwrap()
            .is_some(),
        "sanity: the kv row exists before delete"
    );
    assert_eq!(
        server
            .state
            .db
            .state_row_count(tenant.id, "notes".to_string())
            .await
            .unwrap(),
        2,
        "sanity: the table rows exist before delete"
    );

    let admin = McpClient::with_bearer(&server.base_url, ADMIN_KEY);
    admin
        .tools_call("admin.tenant_delete", json!({"tenant": tenant_ns}))
        .await
        .expect("admin.tenant_delete");

    // The tenant's own row is gone, and the FK's ON DELETE CASCADE has
    // already removed its state rows -- inspect the child tables directly
    // rather than trying to query state through a tenant that no longer
    // authenticates.
    assert!(
        server
            .state
            .db
            .find_tenant_by_namespace(tenant_ns.clone())
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        server
            .state
            .db
            .state_kv_get(tenant.id, "a".to_string())
            .await
            .unwrap()
            .is_none(),
        "tenant_state_kv row must be gone after cascade delete"
    );
    assert_eq!(
        server
            .state
            .db
            .state_row_count(tenant.id, "notes".to_string())
            .await
            .unwrap(),
        0,
        "tenant_state_rows for the deleted tenant must be gone"
    );
    assert!(
        server
            .state
            .db
            .state_table_get(tenant.id, "notes".to_string())
            .await
            .unwrap()
            .is_none(),
        "tenant_state_tables registration must be gone too"
    );
}
