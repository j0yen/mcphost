//! PRD-mcphost-data-quota-in-every-response
//! AC2 -- Given a tenant with 1,200 table rows on a plan with
//! `table_rows_max = 10000`, When `host.table.append` of 200 rows runs, Then
//! the response carries `data.quota.table_rows_max = {used: 1400, limit:
//! 10000, remaining: 8600}` and `data.quota.table_bytes_max` with `used` > 0.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::{Value, json};

fn rows(from: i64, count: i64) -> Value {
    Value::Array((from..from + count).map(|n| json!({"n": n})).collect())
}

#[tokio::test]
async fn append_response_carries_used_limit_remaining_for_touched_ceilings() {
    let server = TestServer::start_with_plans(|catalog| {
        for p in &mut catalog.plans {
            if p.name == "free" {
                p.table_rows_max = 10_000;
            }
        }
    })
    .await;
    let (_ns, key) = signup(&server.base_url, "Quota").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    client
        .tools_call("host.table.create", json!({"name": "t", "columns": {"n": "integer"}}))
        .await
        .expect("create");
    client
        .tools_call("host.table.append", json!({"table": "t", "rows": rows(0, 1200)}))
        .await
        .expect("seed 1200 rows");

    let result = client
        .tools_call("host.table.append", json!({"table": "t", "rows": rows(1200, 200)}))
        .await
        .expect("append 200");
    let body = extract_structured(&result);
    let quota = &body["data"]["quota"];
    assert_eq!(
        quota["table_rows_max"],
        json!({"used": 1400, "limit": 10000, "remaining": 8600}),
        "{body:?}"
    );
    assert!(quota["table_bytes_max"]["used"].as_i64().unwrap_or(0) > 0, "{body:?}");
    // Only the ceilings the table store touches ride on a table verb.
    assert!(quota.get("state_bytes_max").is_none(), "{body:?}");
}
