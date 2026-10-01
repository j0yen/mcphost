//! PRD-mcphost-lineage-blast-radius AC2 (P0) -- Given a table with no consumers, When `host.table.drop` is
//! called without `confirm`, Then it drops as before and no refusal is
//! written.

use crate::common;
use common::{TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn drop_with_no_consumers_drops_and_writes_no_refusal() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "Lineage AC2 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call(
            "host.table.create",
            json!({"name": "scratch", "columns": {"id": "integer"}}),
        )
        .await
        .expect("create scratch");

    let dropped = client
        .tools_call("host.table.drop", json!({"name": "scratch"}))
        .await
        .expect("drop with no consumers must not be refused");
    let structured = extract_structured(&dropped);
    assert_eq!(structured["dropped"], json!(true));

    let tenant = server
        .state
        .db
        .find_tenant_by_namespace(ns)
        .await
        .expect("db")
        .expect("tenant exists");
    let audit = server
        .state
        .db
        .list_tenant_audit_for_subject(tenant.id, "table".to_string(), None, 100)
        .await
        .expect("read audit");
    assert!(
        audit.iter().all(|(_, action, _)| action != "table_drop_refused"),
        "a drop with no consumers must write no refusal: {audit:?}"
    );
}
