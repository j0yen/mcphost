//! PRD-mcphost-table-context-and-sql-passthrough
//! AC6 — Given two tenants that each ran queries, When tenant A calls
//! `host.table.query_log`, Then only A's rows return, newest first, and
//! `limit` 500 is served as 200.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

async fn make_tenant_with_queries(base_url: &str, name: &str, n: usize) -> McpClient {
    let (_ns, key) = signup(base_url, name).await;
    let client = McpClient::with_bearer(base_url, &key);
    client
        .tools_call("host.table.create", json!({"name": "t", "columns": {"n": "integer"}}))
        .await
        .expect("create t");
    client
        .tools_call("host.table.append", json!({"table": "t", "rows": [{"n": 1}]}))
        .await
        .expect("append one row");
    for i in 0..n {
        client
            .tools_call("host.table.query", json!({"sql": format!("SELECT {i} AS marker FROM t")}))
            .await
            .unwrap_or_else(|e| panic!("query {i}: {} {}", e.code, e.message));
    }
    client
}

#[tokio::test]
async fn query_log_is_tenant_isolated_newest_first_and_limit_capped_at_200() {
    let server = TestServer::start().await;
    let client_a = make_tenant_with_queries(&server.base_url, "SqlPass AC6 Tenant A", 210).await;
    let client_b = make_tenant_with_queries(&server.base_url, "SqlPass AC6 Tenant B", 3).await;

    // `limit` 500 is served as 200.
    let log_a = extract_structured(
        &client_a.tools_call("host.table.query_log", json!({"limit": 500})).await.expect("query_log A"),
    );
    let rows_a = log_a["rows"].as_array().expect("rows array");
    assert_eq!(rows_a.len(), 200, "limit 500 must be served as 200: {}", rows_a.len());

    // Newest first: the most recent query (marker 209) leads.
    assert!(
        rows_a[0]["sql"].as_str().unwrap().contains("209 AS marker"),
        "expected newest-first ordering, got: {}",
        rows_a[0]["sql"]
    );
    assert!(
        rows_a[1]["sql"].as_str().unwrap().contains("208 AS marker"),
        "expected newest-first ordering, got: {}",
        rows_a[1]["sql"]
    );

    // Tenant isolation: B's own log shows only its 3 queries, none of A's.
    let log_b = extract_structured(
        &client_b.tools_call("host.table.query_log", json!({})).await.expect("query_log B"),
    );
    let rows_b = log_b["rows"].as_array().expect("rows array");
    assert_eq!(rows_b.len(), 3, "tenant B ran exactly 3 queries: {log_b}");
    for r in rows_b {
        let sql = r["sql"].as_str().unwrap();
        assert!(
            sql.contains("0 AS marker") || sql.contains("1 AS marker") || sql.contains("2 AS marker"),
            "tenant B's log must only contain its own queries, got: {sql}"
        );
    }
}
