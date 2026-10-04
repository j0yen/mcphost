//! PRD-mcphost-table-context-and-sql-passthrough
//! AC7 — Given a tenant with 1,000 log rows, When one more query runs,
//! Then the log holds 1,000 rows, the oldest is gone, and
//! `host.table.schema`'s `bytes_used` includes the log.
//!
//! Driven directly against `mcphost::tables` (bypassing HTTP) so 1,001
//! queries run in-process rather than as 1,001 HTTP round trips.

use mcphost::tables;
use serde_json::json;

use crate::common;
use common::{TestServer, signup};

#[tokio::test]
async fn query_log_caps_at_1000_and_bytes_used_includes_it() {
    let server = TestServer::start().await;
    let (ns, _key) = signup(&server.base_url, "SqlPass AC7 Tenant").await;
    let tenant = server
        .state
        .db
        .find_tenant_by_namespace(ns)
        .await
        .expect("find tenant")
        .expect("tenant exists");

    tables::table_create(&server.state, &tenant, &json!({"name": "t", "columns": {"n": "integer"}}))
        .await
        .expect("create t");
    tables::table_append(&server.state, &tenant, &json!({"table": "t", "rows": [{"n": 1}]}))
        .await
        .expect("append one row");

    let baseline_schema = tables::table_schema(&server.state, &tenant, &json!({"table": "t"}))
        .await
        .expect("schema before any logged query");
    let baseline_bytes = baseline_schema["bytes_used"].as_i64().expect("bytes_used");

    // 1,001 queries: the insert that makes the 1,001st evicts marker 0.
    for i in 0..1_001 {
        tables::table_query(
            &server.state,
            &tenant,
            &json!({"sql": format!("SELECT {i} AS marker FROM t")}),
            None,
        )
        .await
        .unwrap_or_else(|e| panic!("query {i}: {e}"));
    }

    // Page the whole log back (limit caps at 200 per call) and confirm
    // it holds exactly 1,000 rows, newest marker 1000, oldest marker 1
    // (marker 0 evicted).
    let mut all_markers: Vec<i64> = Vec::with_capacity(1_000);
    let mut before_id: Option<i64> = None;
    loop {
        let mut args = json!({"limit": 200});
        if let Some(b) = before_id {
            args["before_id"] = json!(b);
        }
        let page = tables::table_query_log(&server.state, &tenant, &args).await.expect("query_log page");
        let rows = page["rows"].as_array().expect("rows array");
        if rows.is_empty() {
            break;
        }
        for r in rows {
            let sql = r["sql"].as_str().expect("sql");
            let marker: i64 = sql
                .strip_prefix("SELECT ")
                .and_then(|s| s.split(' ').next())
                .and_then(|s| s.parse().ok())
                .unwrap_or_else(|| panic!("unexpected log sql shape: {sql}"));
            all_markers.push(marker);
        }
        before_id = Some(rows.last().unwrap()["id"].as_i64().expect("id"));
    }

    assert_eq!(all_markers.len(), 1_000, "the log must hold exactly 1,000 rows");
    assert_eq!(*all_markers.first().unwrap(), 1_000, "newest first, newest marker is 1000");
    assert_eq!(*all_markers.last().unwrap(), 1, "oldest surviving marker is 1 -- marker 0 was evicted");
    assert!(!all_markers.contains(&0), "marker 0 (the 1001st-oldest) must have been evicted");

    let after_schema = tables::table_schema(&server.state, &tenant, &json!({"table": "t"}))
        .await
        .expect("schema after 1000 logged queries");
    let after_bytes = after_schema["bytes_used"].as_i64().expect("bytes_used");
    assert!(
        after_bytes > baseline_bytes,
        "bytes_used must grow to include the query log's own rows: before={baseline_bytes} after={after_bytes}"
    );
}
