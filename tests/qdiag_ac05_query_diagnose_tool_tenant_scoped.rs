//! PRD-mcphost-query-diagnosis
//! AC5 — Given a log row id, When `host.table.query_diagnose` is called by
//! the owning tenant, Then it returns that row's diagnosis; when called by
//! another tenant, Then `not_found`.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn query_diagnose_returns_for_owner_and_not_found_for_another_tenant() {
    let server = TestServer::start().await;
    let (_ns_a, key_a) = signup(&server.base_url, "QDiag AC5 Tenant A").await;
    let (_ns_b, key_b) = signup(&server.base_url, "QDiag AC5 Tenant B").await;
    let client_a = McpClient::with_bearer(&server.base_url, &key_a);
    let client_b = McpClient::with_bearer(&server.base_url, &key_b);

    client_a
        .tools_call(
            "host.table.create",
            json!({"name": "expenses", "columns": {"amount": "real"}}),
        )
        .await
        .expect("create a");

    client_a
        .tools_call("host.table.query", json!({"sql": "SELECT amout FROM expenses"}))
        .await
        .expect_err("amout is a typo");

    let log = extract_structured(
        &client_a.tools_call("host.table.query_log", json!({"limit": 1})).await.expect("query_log"),
    );
    let log_id = log["rows"][0]["id"].as_i64().expect("log_id");

    let diag = extract_structured(
        &client_a
            .tools_call("host.table.query_diagnose", json!({"log_id": log_id}))
            .await
            .expect("query_diagnose by owner"),
    );
    assert_eq!(diag["diagnosis"]["identifiers"][0]["term"], "amout", "diag: {diag}");
    assert_eq!(diag["hint"].as_str().map(|h| h.contains("amount")), Some(true), "diag: {diag}");

    // Tenant B needs its own table/tenant file to exist for the lookup to
    // even open a connection; the row itself still must not be visible.
    client_b
        .tools_call("host.table.create", json!({"name": "other", "columns": {"x": "text"}}))
        .await
        .expect("create b");
    let err = client_b
        .tools_call("host.table.query_diagnose", json!({"log_id": log_id}))
        .await
        .expect_err("tenant B must not see tenant A's log row");
    assert_eq!(err.error_code.as_deref(), Some("not_found"), "err: {err:?}");
}
