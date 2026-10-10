//! AC2 — Given a tenant at its `calls_per_day` ceiling, When it calls any
//! tool, Then the refusal response is unchanged, a row with
//! `error_code = calls_per_day` is written, and
//! `count_calls_since(ok_only = true)` is unchanged.

use crate::common;
use common::{McpClient, TestServer, signup};
use serde_json::json;

#[tokio::test]
async fn calls_per_day_refusal_writes_row_and_leaves_quota_count() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "Refused Quota Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "echoer", "kind": "echo", "spec": {"schema": {"type": "object"}}}),
        )
        .await
        .expect("publish");
    let tenant = server
        .state
        .db
        .find_tenant_by_namespace(ns.clone())
        .await
        .expect("db query")
        .expect("tenant");
    for _ in 0..500 {
        server
            .state
            .db
            .record_call(tenant.id, "echoer".to_string(), 1, true, None, None, None, "ok", "external".to_string(), None)
            .await
            .expect("seed call");
    }
    let midnight = mcphost::state::utc_midnight_unix(mcphost::state::now_unix());
    let before = server.state.db.count_calls_since(tenant.id, midnight, true).await.unwrap();

    let err = client
        .tools_call(&format!("{ns}.echoer"), json!({}))
        .await
        .expect_err("over the daily quota");
    assert_eq!(err.error_code.as_deref(), Some("quota_exceeded"));
    assert_eq!(err.data["limit"]["name"], json!("calls_per_day"));

    let conn = rusqlite::Connection::open(server.data_dir.0.join("mcphost.db")).expect("open raw db");
    let (n, ok, outcome): (i64, i64, String) = conn
        .query_row(
            "SELECT COUNT(*), MAX(ok), MAX(outcome) FROM calls WHERE tenant_id = ?1 AND error_code = 'calls_per_day'",
            [tenant.id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .expect("query");
    assert_eq!((n, ok, outcome.as_str()), (1, 0, "refused"));

    let after = server.state.db.count_calls_since(tenant.id, midnight, true).await.unwrap();
    assert_eq!(before, after, "a refused row must not raise calls_per_day usage");
}
