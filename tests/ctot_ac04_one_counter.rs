//! PRD-mcphost-count-and-total-semantics
//! AC4 -- Given fixture tables of 9 999 and 10 001 rows, then
//! `host.table.query` page total/total_estimate, `host.table.count`, and
//! `host.table.list.rows` all agree (exact at 9 999; `exact: false` at
//! 10 001).

use crate::common;
use crate::ctot_ac02_page_total_exact_then_estimate::{fill_table, start_server};
use common::{McpClient, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn ctot_ac04_one_counter() {
    let server = start_server().await;
    let (_ns, key) = signup(&server.base_url, "Ctot AC4 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    // ── 9 999 rows — exact ───────────────────────────────────────────────────
    fill_table(&client, "small_t", 9_999).await;

    // host.table.query page total
    let page = client
        .tools_call(
            "host.table.query",
            json!({"sql": "SELECT n FROM small_t ORDER BY n", "limit": 10}),
        )
        .await
        .expect("query ok");
    let page = extract_structured(&page);
    assert_eq!(page["total"], json!(9999), "query total: {page:?}");
    assert!(page.get("total_estimate").is_none(), "no estimate: {page:?}");

    // host.table.count
    let cnt = client
        .tools_call("host.table.count", json!({"table": "small_t"}))
        .await
        .expect("count ok");
    let cnt = extract_structured(&cnt);
    assert_eq!(cnt["count"], json!(9999), "count: {cnt:?}");
    assert_eq!(cnt["exact"], json!(true), "count exact: {cnt:?}");

    // host.table.list .rows field (the "rows" entry for small_t)
    let list = client
        .tools_call("host.table.list", json!({}))
        .await
        .expect("list ok");
    let list = extract_structured(&list);
    let small_entry = list["tables"].as_array().expect("tables")
        .iter().find(|t| t["name"] == json!("small_t"))
        .expect("small_t in list");
    assert_eq!(small_entry["rows"], json!(9999), "list rows: {small_entry:?}");
    assert_eq!(small_entry["exact"], json!(true), "list exact: {small_entry:?}");

    // ── 10 001 rows — estimate ───────────────────────────────────────────────
    fill_table(&client, "big_t", 10_001).await;

    // host.table.query page — estimate
    let page = client
        .tools_call(
            "host.table.query",
            json!({"sql": "SELECT n FROM big_t ORDER BY n", "limit": 10}),
        )
        .await
        .expect("query big ok");
    let page = extract_structured(&page);
    assert!(page["total_estimate"].as_u64().expect("total_estimate") >= 10_001, "query estimate: {page:?}");
    assert_eq!(page["exact"], json!(false), "query exact=false: {page:?}");
    assert!(page.get("total").is_none(), "no total: {page:?}");

    // host.table.count — estimate
    let cnt = client
        .tools_call("host.table.count", json!({"table": "big_t"}))
        .await
        .expect("count big ok");
    let cnt = extract_structured(&cnt);
    assert!(cnt["count"].as_u64().expect("count") >= 10_001, "big count: {cnt:?}");
    assert_eq!(cnt["exact"], json!(false), "big count exact=false: {cnt:?}");

    // host.table.list .rows field for big_t — estimate
    let list = client
        .tools_call("host.table.list", json!({}))
        .await
        .expect("list big ok");
    let list = extract_structured(&list);
    let big_entry = list["tables"].as_array().expect("tables")
        .iter().find(|t| t["name"] == json!("big_t"))
        .expect("big_t in list");
    assert!(big_entry["rows"].as_u64().expect("rows") >= 10_001, "list rows estimate: {big_entry:?}");
    assert_eq!(big_entry["exact"], json!(false), "list exact=false: {big_entry:?}");
}
