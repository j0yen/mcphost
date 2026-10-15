//! PRD-mcphost-call-audit-export
//! AC2 (P0) — Given 1 200 rows, When `host.calls.list {limit: 500}` pages
//! with `cursor` three times, Then 1 200 distinct rows return newest first
//! and the fourth page is empty.

use crate::common::{McpClient, TestServer, extract_structured, signup};
use mcphost::paged::Cursor;
use serde_json::{Value, json};

const ROWS: i64 = 1_200;

#[tokio::test]
async fn cae_ac02_three_pages_of_500_return_1200_distinct_rows_newest_first() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "Paging Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    let tenant = server.state.db.find_tenant_by_namespace(ns).await.expect("find").expect("tenant");
    let now = mcphost::state::now_unix();
    for i in 0..ROWS {
        server
            .state
            .db
            .insert_call_row_for_test_full(tenant.id, format!("t{i:04}"), None, None, true, 1, now - 60)
            .await
            .expect("seed row");
    }

    let mut tools: Vec<String> = Vec::new();
    let mut cursor: Option<String> = None;
    let mut page_sizes = Vec::new();
    for _ in 0..3 {
        let mut args = json!({"since": "-1h", "limit": 500});
        if let Some(c) = &cursor {
            args["cursor"] = json!(c);
        }
        let page = extract_structured(&client.tools_call("host.calls.list", args).await.expect("page"));
        let rows = page["calls"].as_array().expect("calls");
        page_sizes.push(rows.len());
        tools.extend(rows.iter().map(|r| r["tool"].as_str().expect("tool").to_string()));
        cursor = page.get("next_cursor").and_then(Value::as_str).map(str::to_string);
        if page_sizes.len() < 3 {
            assert!(cursor.is_some(), "page {} must carry next_cursor: {page}", page_sizes.len());
        } else {
            assert!(cursor.is_none(), "the third page is the last: {page}");
        }
    }
    assert_eq!(page_sizes, vec![500, 500, 200]);
    assert_eq!(tools.len() as i64, ROWS);
    let distinct: std::collections::BTreeSet<&String> = tools.iter().collect();
    assert_eq!(distinct.len() as i64, ROWS, "every row returns exactly once");
    let mut newest_first = tools.clone();
    newest_first.sort_by(|a, b| b.cmp(a));
    assert_eq!(tools, newest_first, "rows come back newest first");

    // The fourth page: a signed cursor positioned past the oldest row (id 1)
    // yields nothing, and carries no further cursor.
    let past_the_end = Cursor::new(1, "1")
        .with_order("newest_first")
        .encode(&server.state.session_bindings, tenant.id);
    let fourth = extract_structured(
        &client
            .tools_call("host.calls.list", json!({"since": "-1h", "limit": 500, "cursor": past_the_end}))
            .await
            .expect("fourth page"),
    );
    assert_eq!(fourth["calls"], json!([]), "{fourth}");
    assert!(fourth.get("next_cursor").is_none(), "{fourth}");
}
