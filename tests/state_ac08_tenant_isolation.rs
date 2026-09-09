//! PRD-mcphost-tenant-state
//! AC8 — Given two tenants, When tenant A's tool calls `mcphost.state.get`
//! for a key tenant B set, Then it reads nothing from B's store (the store
//! is scoped by tenant id, verified by a test that inspects rows). This
//! suite exercises the same scoping through the `host.state.*`
//! control-plane surface (the sandbox's `mcphost.state` module is not part
//! of this increment -- see `tenant_state.rs`'s module doc) plus a direct
//! row inspection, and extends the same isolation check to tables.

mod common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn a_tenants_state_is_invisible_to_another_tenant() {
    let server = TestServer::start().await;
    let (ns_a, key_a) = signup(&server.base_url, "Tenant A").await;
    let (_ns_b, key_b) = signup(&server.base_url, "Tenant B").await;
    let client_a = McpClient::with_bearer(&server.base_url, &key_a);
    let client_b = McpClient::with_bearer(&server.base_url, &key_b);

    client_a
        .tools_call("host.state.set", json!({"key": "secret", "value": 1}))
        .await
        .expect("A sets a key");

    let got_by_b = extract_structured(
        &client_b
            .tools_call("host.state.get", json!({"key": "secret"}))
            .await
            .expect("B's get must not itself error"),
    );
    assert_eq!(
        got_by_b["found"],
        json!(false),
        "B must not see A's key: {got_by_b:?}"
    );

    let listed_by_b = extract_structured(
        &client_b
            .tools_call("host.state.list", json!({}))
            .await
            .expect("B's list"),
    );
    assert_eq!(
        listed_by_b["keys"].as_array().expect("keys array").len(),
        0,
        "B's own namespace must be empty: {listed_by_b:?}"
    );

    // Direct row inspection: A's row exists, scoped to A's tenant id.
    let tenant_a = server
        .state
        .db
        .find_tenant_by_namespace(ns_a)
        .await
        .unwrap()
        .expect("tenant a");
    assert!(
        server
            .state
            .db
            .state_kv_get(tenant_a.id, "secret".to_string())
            .await
            .unwrap()
            .is_some(),
        "sanity: A's own row exists"
    );
}

#[tokio::test]
async fn a_tenants_tables_are_invisible_to_another_tenant() {
    let server = TestServer::start().await;
    let (_ns_a, key_a) = signup(&server.base_url, "Tenant A").await;
    let (_ns_b, key_b) = signup(&server.base_url, "Tenant B").await;
    let client_a = McpClient::with_bearer(&server.base_url, &key_a);
    let client_b = McpClient::with_bearer(&server.base_url, &key_b);

    client_a
        .tools_call(
            "host.state.table_create",
            json!({"name": "notes", "schema": {"body": "text"}}),
        )
        .await
        .expect("A declares a table");
    client_a
        .tools_call(
            "host.state.insert",
            json!({"table": "notes", "rows": [{"body": "A's note"}]}),
        )
        .await
        .expect("A inserts a row");

    let err = client_b
        .tools_call("host.state.query", json!({"table": "notes"}))
        .await
        .expect_err("B must not see a table it never declared");
    assert_eq!(err.error_code.as_deref(), Some("state_table_not_found"));
}
