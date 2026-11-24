//! PRD-mcphost-response-size-budget-explicit-v2
//! AC5 (P1) -- Given a 5 MiB table and a 32 KiB plan budget, When
//! `host.table.query {sql: "SELECT * FROM t", limit: 1000}` runs and each
//! `continue_with` is followed, Then every page is within the budget and ends
//! on a whole row and the walk visits every row once; Given an empty table,
//! Then `rows: []` with no marker.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::{Value, json};

const BUDGET: usize = 32 * 1024;
const ROWS: i64 = 640;
const PAD: usize = 8 * 1024;

async fn table_server() -> (TestServer, McpClient) {
    let server = TestServer::start_with_plans(|catalog| {
        for plan in &mut catalog.plans {
            plan.response_budget_bytes = BUDGET as i64;
        }
    })
    .await;
    let (_ns, key) = signup(&server.base_url, "Rszb AC5").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    (server, client)
}

#[tokio::test]
async fn rszb_ac05_a_five_mib_table_walks_by_offset_within_the_budget() {
    let (_server, client) = table_server().await;
    client
        .tools_call("host.table.create", json!({"name": "t", "columns": {"id": "integer", "pad": "text"}}))
        .await
        .expect("create");
    let pad = "z".repeat(PAD);
    for chunk in 0..(ROWS / 40) {
        let rows: Vec<Value> = (0..40).map(|i| json!({"id": chunk * 40 + i, "pad": pad})).collect();
        client.tools_call("host.table.append", json!({"table": "t", "rows": rows})).await.expect("append");
    }

    let base = json!({"sql": "SELECT * FROM t", "limit": 1000});
    let mut args = base.clone();
    let mut seen: Vec<i64> = Vec::new();
    let mut cut_pages = 0;
    for page_no in 0..400 {
        let result = client.tools_call("host.table.query", args.clone()).await.unwrap_or_else(|e| panic!("page {page_no}: {e:?}"));
        let body = extract_structured(&result);
        let rows = body["rows"].as_array().expect("rows");
        let size = serde_json::to_vec(rows).expect("serialize").len();
        assert!(size <= BUDGET || rows.len() == 1, "page {page_no}: {size} bytes in {} rows", rows.len());
        assert_eq!(body["total"], json!(ROWS), "total names every row that exists: {}", body["total"]);
        for row in rows {
            assert_eq!(row["pad"].as_str().map(str::len), Some(PAD), "a page ends on whole rows");
            seen.push(row["id"].as_i64().expect("id"));
        }
        match result["_meta"]["mcphost"].get("truncated") {
            Some(marker) => {
                cut_pages += 1;
                let next = &marker["continue_with"];
                assert_eq!(next["verb"], json!("host.table.query"), "{marker}");
                let mut expected = base.clone();
                expected["offset"] = json!(seen.len());
                assert_eq!(next["args"], expected, "the original sql and limit, offset past the rows sent");
                assert_eq!(marker["bytes"], json!(size), "{}", marker["bytes"]);
                args = next["args"].clone();
            }
            None => {
                assert_eq!(seen.len() as i64, ROWS, "the last page ends the walk");
                break;
            }
        }
    }
    assert!(cut_pages >= 100, "a 5 MiB table at 32 KiB is many pages, got {cut_pages}");
    assert_eq!(seen, (0..ROWS).collect::<Vec<_>>(), "every row once, in order");
}

#[tokio::test]
async fn rszb_ac05_an_empty_table_has_no_marker_and_offset_is_validated() {
    let (_server, client) = table_server().await;
    client
        .tools_call("host.table.create", json!({"name": "empty_t", "columns": {"id": "integer"}}))
        .await
        .expect("create");
    let result = client.tools_call("host.table.query", json!({"sql": "SELECT * FROM empty_t", "limit": 1000})).await.expect("query");
    assert_eq!(extract_structured(&result)["rows"], json!([]), "{result:?}");
    assert!(result["_meta"].get("mcphost").is_none_or(Value::is_null), "{result:?}");

    // `offset` composes with ORDER BY and a limit, and a small page carries no marker.
    client
        .tools_call("host.table.append", json!({"table": "empty_t", "rows": (0..5).map(|i| json!({"id": i})).collect::<Vec<_>>()}))
        .await
        .expect("append");
    let page = client
        .tools_call("host.table.query", json!({"sql": "SELECT id FROM empty_t ORDER BY id DESC", "limit": 2, "offset": 1}))
        .await
        .expect("offset page");
    assert_eq!(extract_structured(&page)["rows"], json!([{"id": 3}, {"id": 2}]), "{page:?}");
    assert!(page["_meta"].get("mcphost").is_none_or(Value::is_null), "{page:?}");

    for bad in [json!(-1), json!("x"), json!(1.5)] {
        let err = client
            .tools_call("host.table.query", json!({"sql": "SELECT * FROM empty_t", "offset": bad}))
            .await
            .expect_err("a bad offset is refused");
        assert_eq!(err.error_code.as_deref(), Some("args_invalid"), "{bad}: {err:?}");
    }
}
