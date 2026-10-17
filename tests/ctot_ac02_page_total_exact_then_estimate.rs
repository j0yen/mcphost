//! PRD-mcphost-count-and-total-semantics
//! AC2 -- Given 9 999 fixture rows, When `host.table.query(limit=50)` returns
//! page 1, Then `total: 9999` and no `total_estimate`; Given 10 001 rows, Then
//! `total_estimate` is present, `exact: false`, and no `total` key.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::{Value, json};

/// A server whose free plan holds both fixture tables.
pub async fn start_server() -> TestServer {
    TestServer::start_with_plans(|catalog| {
        for p in &mut catalog.plans {
            if p.name == "free" {
                p.table_rows_max = 100_000;
            }
        }
    })
    .await
}

/// Create `name` and fill it with `n` rows, 1 000 per append.
pub async fn fill_table(client: &McpClient, name: &str, n: usize) {
    client
        .tools_call("host.table.create", json!({"name": name, "columns": {"n": "integer"}}))
        .await
        .expect("table create ok");
    let mut done = 0;
    while done < n {
        let end = (done + 1000).min(n);
        let rows: Vec<Value> = (done..end).map(|i| json!({"n": i as i64})).collect();
        client
            .tools_call("host.table.append", json!({"table": name, "rows": rows}))
            .await
            .expect("append ok");
        done = end;
    }
}

#[tokio::test]
async fn ctot_ac02_page_total_exact_then_estimate() {
    let server = start_server().await;
    let (_ns, key) = signup(&server.base_url, "Ctot AC2 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    fill_table(&client, "small_t", 9_999).await;
    let page = client
        .tools_call("host.table.query", json!({"sql": "SELECT n FROM small_t ORDER BY n", "limit": 50}))
        .await
        .expect("query ok");
    let page = extract_structured(&page);
    assert_eq!(page["rows"].as_array().expect("rows").len(), 50, "{page:?}");
    assert_eq!(page["total"], json!(9999), "{page:?}");
    assert!(page.get("total_estimate").is_none(), "{page:?}");

    fill_table(&client, "big_t", 10_001).await;
    let page = client
        .tools_call("host.table.query", json!({"sql": "SELECT n FROM big_t ORDER BY n", "limit": 50}))
        .await
        .expect("query ok");
    let page = extract_structured(&page);
    assert_eq!(page["rows"].as_array().expect("rows").len(), 50, "{page:?}");
    assert!(page["total_estimate"].as_u64().expect("total_estimate") >= 10_001, "{page:?}");
    assert_eq!(page["exact"], json!(false), "{page:?}");
    assert!(page.get("total").is_none(), "{page:?}");
}
