//! PRD-mcphost-result-handles
//! AC11 (P1) -- Given two tenants, When tenant A references tenant B's
//! handle name, Then `handle_not_found` and no rows.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn tenant_a_referencing_tenant_bs_handle_is_handle_not_found() {
    let server = TestServer::start().await;
    let (_ns_a, key_a) = signup(&server.base_url, "ResultHandles AC11 Tenant A").await;
    let (_ns_b, key_b) = signup(&server.base_url, "ResultHandles AC11 Tenant B").await;
    let client_a = McpClient::with_bearer(&server.base_url, &key_a);
    let client_b = McpClient::with_bearer(&server.base_url, &key_b);

    client_b
        .tools_call("host.table.create", json!({"name": "secrets", "columns": {"v": "text"}}))
        .await
        .expect("create for tenant b");
    client_b
        .tools_call("host.table.append", json!({"table": "secrets", "rows": [{"v": "top secret"}]}))
        .await
        .expect("append for tenant b");
    let materialize_b = extract_structured(
        &client_b
            .tools_call("host.table.query", json!({"sql": "SELECT * FROM secrets", "handle": true}))
            .await
            .expect("tenant b materializes"),
    );
    let handle_b = materialize_b["handle"].as_str().expect("handle").to_string();

    // Sanity: tenant B's own handle is live for tenant B.
    let own = extract_structured(
        &client_b
            .tools_call("host.table.query", json!({"sql": format!("SELECT * FROM {handle_b}")}))
            .await
            .expect("tenant b queries its own handle"),
    );
    assert_eq!(own["rows"].as_array().unwrap().len(), 1, "own: {own}");

    let err = client_a
        .tools_call("host.table.query", json!({"sql": format!("SELECT * FROM {handle_b}")}))
        .await
        .expect_err("tenant A must not see tenant B's handle");
    assert_eq!(err.error_code.as_deref(), Some("handle_not_found"), "err: {err:?}");
    assert_eq!(err.data["handle"], handle_b, "err: {err:?}");
}
