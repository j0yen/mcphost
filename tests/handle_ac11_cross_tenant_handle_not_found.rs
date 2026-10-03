//! PRD-mcphost-result-handles
//! AC11 -- Given two tenants, When tenant A references tenant B's handle
//! name, Then `handle_not_found` and no rows.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn tenant_a_referencing_tenant_bs_handle_reads_as_not_found() {
    let server = TestServer::start().await;
    let (_ns_a, key_a) = signup(&server.base_url, "Handle AC11 Tenant A").await;
    let (_ns_b, key_b) = signup(&server.base_url, "Handle AC11 Tenant B").await;
    let client_a = McpClient::with_bearer(&server.base_url, &key_a);
    let client_b = McpClient::with_bearer(&server.base_url, &key_b);

    client_b
        .tools_call("host.table.create", json!({"name": "secrets", "columns": {"v": "text"}}))
        .await
        .expect("create for tenant b");
    client_b
        .tools_call("host.table.append", json!({"table": "secrets", "rows": [{"v": "shh"}]}))
        .await
        .expect("append for tenant b");
    let summary = extract_structured(
        &client_b
            .tools_call("host.table.query", json!({"sql": "SELECT * FROM secrets", "handle": true}))
            .await
            .expect("materialize handle for tenant b"),
    );
    let handle = summary["handle"].as_str().expect("handle name").to_string();

    // Sanity check: tenant B can query its own handle.
    let own = extract_structured(
        &client_b
            .tools_call("host.table.query", json!({"sql": format!("SELECT * FROM {handle}")}))
            .await
            .expect("tenant b can query its own handle"),
    );
    assert_eq!(own["rows"].as_array().unwrap().len(), 1, "own: {own}");

    // Tenant A referencing the exact same handle name -- structurally,
    // this tenant's own per-tenant file simply has no such row, same
    // isolation `tables.rs`'s module doc already gives declared tables.
    let err = client_a
        .tools_call("host.table.query", json!({"sql": format!("SELECT * FROM {handle}")}))
        .await
        .expect_err("tenant A must not see tenant B's handle");
    assert_eq!(err.error_code.as_deref(), Some("handle_not_found"), "error: {err:?}");
    assert!(err.message.contains(&handle), "error message must name the handle: {}", err.message);

    // Tenant A's own host.table.handles must never list it either.
    let listed = extract_structured(
        &client_a.tools_call("host.table.handles", json!({})).await.expect("host.table.handles for tenant a"),
    );
    assert!(
        listed["handles"].as_array().unwrap().is_empty(),
        "tenant A must not see tenant B's handle in its own listing: {listed}"
    );
}
