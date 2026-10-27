//! PRD-mcphost-table-import-from-url
//! AC4 (P0) — Given `csv_text` of 50 lines with one malformed row, When
//! appended to an existing table, Then 49 rows land, `rows_skipped 1`, and the
//! first skip reason names the line number and column.

use crate::common;

use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::{Value, json};

async fn call(client: &McpClient, tool: &str, args: Value) -> Value {
    extract_structured(&client.tools_call(tool, args).await.unwrap_or_else(|e| panic!("{tool}: {e:?}")))
}

async fn orders_table(client: &McpClient) {
    call(client, "host.table.create", json!({"name": "orders", "columns": {"id": "integer", "name": "text", "qty": "integer"}})).await;
}

/// A header and 50 data lines (file lines 2..=51); data line `bad_line` has a non-numeric `qty`.
fn csv_with_one_bad_row(bad_line: usize) -> String {
    let mut out = String::from("id,name,qty\n");
    for i in 1..=50usize {
        let line = i + 1;
        let qty = if line == bad_line { "many".to_string() } else { (i % 7).to_string() };
        out.push_str(&format!("{i},item {i},{qty}\n"));
    }
    out
}

#[tokio::test]
async fn tblurl_ac04_one_bad_row_of_fifty_is_skipped_and_named() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "TBLURL AC4").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    orders_table(&client).await;

    let out = call(&client, "host.table.append", json!({"table": "orders", "csv_text": csv_with_one_bad_row(27)})).await;
    assert_eq!(out["rows_imported"], json!(49), "{out}");
    assert_eq!(out["appended"], json!(49), "{out}");
    assert_eq!(out["rows_skipped"], json!(1), "{out}");
    assert_eq!(out["source"], json!("inline"), "{out}");
    assert!(out.get("source_host").is_none(), "{out}");
    let reasons = out["skip_reasons"].as_array().expect("skip_reasons");
    assert_eq!(reasons.len(), 1, "{out}");
    let first = reasons[0].as_str().expect("reason");
    assert!(first.contains("line 27"), "names the line: {first}");
    assert!(first.contains("qty"), "names the column: {first}");

    let count = call(&client, "host.table.count", json!({"table": "orders"})).await;
    assert_eq!(count["count"], json!(49), "{count}");
    let gone = call(&client, "host.table.query", json!({"sql": "SELECT COUNT(*) AS n FROM orders WHERE id = 26"})).await;
    assert_eq!(gone["rows"][0]["n"], json!(0), "the bad row (id 26) did not land: {gone}");
}

#[tokio::test]
async fn tblurl_ac04_only_the_first_three_skip_reasons_are_returned() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "TBLURL AC4 reasons").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    orders_table(&client).await;

    let csv = "id,name,qty\n1,a,1\n2,b,x\n3,c,y\n4,d\n5,e,z\n6,f,2\n";
    let out = call(&client, "host.table.append", json!({"table": "orders", "csv_text": csv})).await;
    assert_eq!(out["rows_imported"], json!(2), "{out}");
    assert_eq!(out["rows_skipped"], json!(4), "{out}");
    let reasons: Vec<&str> = out["skip_reasons"].as_array().unwrap().iter().filter_map(Value::as_str).collect();
    assert_eq!(reasons.len(), 3, "{reasons:?}");
    assert!(reasons[0].contains("line 3") && reasons[0].contains("qty"), "{reasons:?}");
    assert!(reasons[2].contains("line 5"), "the short row is a field-count skip: {reasons:?}");
}

#[tokio::test]
async fn tblurl_ac04_no_row_fitting_the_table_is_import_schema_mismatch() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "TBLURL AC4 mismatch").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    orders_table(&client).await;

    let err = client.tools_call("host.table.append", json!({"table": "orders", "csv_text": "sku,price\nA,1\nB,2\n"})).await.expect_err("mismatch");
    assert_eq!(err.error_code.as_deref(), Some("import_schema_mismatch"), "{err:?}");
    assert_eq!(err.data["expected"], json!(["id", "name", "qty"]), "{:?}", err.data);

    let err = client.tools_call("host.table.append", json!({"table": "orders", "csv_text": "id,name,qty\n1,a,x\n2,b,y\n"})).await.expect_err("all rows bad");
    assert_eq!(err.error_code.as_deref(), Some("import_schema_mismatch"), "{err:?}");
    assert_eq!(err.data["rows_skipped"], json!(2), "{:?}", err.data);
    let count = call(&client, "host.table.count", json!({"table": "orders"})).await;
    assert_eq!(count["count"], json!(0), "{count}");
}

#[tokio::test]
async fn tblurl_ac04_inline_text_over_one_megabyte_and_unclosed_quotes_are_refused() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "TBLURL AC4 limits").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    orders_table(&client).await;
    let tenant = server.state.db.find_tenant_by_namespace(ns.clone()).await.expect("lookup").expect("tenant");

    // The transport refuses a request body past 1 MiB before the handler sees
    // it, so the handler's own 1 MB ceiling is exercised in process.
    let big = format!("id,name,qty\n{}", "1,aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa,1\n".repeat(10_000));
    assert!(big.len() > 1024 * 1024);
    match mcphost::tables::table_append(&server.state, &tenant, &json!({"table": "orders", "csv_text": big})).await {
        Err(mcphost::errors::AppError::Structured { code, data, .. }) => {
            assert_eq!(code, "import_too_large");
            assert_eq!(data["bytes"], json!(1024 * 1024), "{data}");
        }
        other => panic!("expected import_too_large, got {other:?}"),
    }

    let err = client.tools_call("host.table.append", json!({"table": "orders", "csv_text": "id,name,qty\n1,\"open,1\n"})).await.expect_err("unclosed");
    assert_eq!(err.error_code.as_deref(), Some("import_parse_failed"), "{err:?}");
    assert_eq!(err.data["line"], json!(2), "{:?}", err.data);
}

#[tokio::test]
async fn tblurl_ac04_json_lines_text_is_sniffed_and_coerced_to_the_schema() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "TBLURL AC4 jsonl").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    orders_table(&client).await;

    let text = "{\"id\":1,\"name\":\"a\",\"qty\":3}\n{\"id\":\"2\",\"name\":\"b\",\"qty\":null}\nnot json\n{\"id\":3,\"name\":\"c\",\"qty\":\"x\"}\n";
    let out = call(&client, "host.table.append", json!({"table": "orders", "csv_text": text})).await;
    assert_eq!(out["rows_imported"], json!(2), "{out}");
    assert_eq!(out["rows_skipped"], json!(2), "{out}");
    let reasons = out["skip_reasons"].to_string();
    assert!(reasons.contains("line 3") && reasons.contains("line 4") && reasons.contains("qty"), "{reasons}");
}
