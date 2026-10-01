//! PRD-mcphost-table-context-and-sql-passthrough
//! AC6 — Given two tenants that each ran queries, When tenant A calls
//! `host.table.query_log`, Then only A's rows return, newest first, and
//! `limit` 500 is served as 200.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn query_log_is_tenant_isolated_newest_first_and_limit_capped() {
    let server = TestServer::start().await;
    let (_ns_a, key_a) = signup(&server.base_url, "SqlPass AC6 Tenant A").await;
    let (_ns_b, key_b) = signup(&server.base_url, "SqlPass AC6 Tenant B").await;
    let client_a = McpClient::with_bearer(&server.base_url, &key_a);
    let client_b = McpClient::with_bearer(&server.base_url, &key_b);

    client_a
        .tools_call("host.table.create", json!({"name": "t", "columns": {"x": "integer"}}))
        .await
        .expect("create a");
    client_b
        .tools_call("host.table.create", json!({"name": "t", "columns": {"x": "integer"}}))
        .await
        .expect("create b");

    // Tenant A runs 3 distinct queries; tenant B runs a different one.
    for n in 1..=3 {
        client_a
            .tools_call("host.table.query", json!({"sql": format!("SELECT {n} AS n FROM t")}))
            .await
            .expect("tenant a query");
    }
    client_b
        .tools_call("host.table.query", json!({"sql": "SELECT 99 AS n FROM t"}))
        .await
        .expect("tenant b query");

    let log_a = extract_structured(
        &client_a.tools_call("host.table.query_log", json!({})).await.expect("query_log a"),
    );
    let entries_a = log_a["entries"].as_array().expect("entries");
    assert_eq!(entries_a.len(), 3, "tenant A must see exactly its own 3 rows: {log_a}");
    assert!(
        entries_a.iter().all(|e| !e["sql"].as_str().unwrap_or_default().contains("99")),
        "tenant A must never see tenant B's query: {log_a}"
    );
    // Newest first.
    assert_eq!(entries_a[0]["sql"], "SELECT 3 AS n FROM t", "log_a: {log_a}");
    assert_eq!(entries_a[2]["sql"], "SELECT 1 AS n FROM t", "log_a: {log_a}");

    let log_b = extract_structured(
        &client_b.tools_call("host.table.query_log", json!({})).await.expect("query_log b"),
    );
    let entries_b = log_b["entries"].as_array().expect("entries");
    assert_eq!(entries_b.len(), 1, "log_b: {log_b}");
    assert_eq!(entries_b[0]["sql"], "SELECT 99 AS n FROM t", "log_b: {log_b}");

    // limit 500 is served as 200 -- needs more than 200 rows logged to be
    // a real proof of the cap, not just of "fewer rows than I asked for."
    for n in 4..=210 {
        client_a
            .tools_call("host.table.query", json!({"sql": format!("SELECT {n} AS n FROM t")}))
            .await
            .expect("tenant a filler query");
    }
    let capped = extract_structured(
        &client_a
            .tools_call("host.table.query_log", json!({"limit": 500}))
            .await
            .expect("query_log limit 500"),
    );
    assert_eq!(capped["entries"].as_array().expect("entries").len(), 200, "capped: {capped}");
}
