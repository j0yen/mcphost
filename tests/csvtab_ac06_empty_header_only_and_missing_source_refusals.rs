//! PRD-mcphost-csv-to-table-one-call
//! AC6 -- Given an empty file, a header-only file, and a file with no `blob`
//! or `doc` argument, When loaded, Then the refusals are `csv_empty`,
//! `csv_no_rows` (table still created with 0 rows) and `args_invalid` naming
//! the two arguments.

use crate::common;
use mcphost::{csvtab, docs, tables};
use serde_json::json;

fn scratch_dir(label: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("mcphost-csvtab-{label}-{}-{}", std::process::id(), mcphost::state::now_unix_ms()));
    std::fs::create_dir_all(&dir).expect("scratch dir");
    dir
}

#[tokio::test]
async fn empty_header_only_and_no_source_each_get_their_own_refusal() {
    let dir = scratch_dir("ac06");
    let state = common::bare_state(&dir).await;
    let tenant = common::bare_tenant(&state, "csvtab-ac06").await;
    docs::doc_put(&state, &tenant, &json!({"name": "empty.csv", "mime": "text/csv", "content": ""})).await.expect("put empty");
    docs::doc_put(&state, &tenant, &json!({"name": "head.csv", "mime": "text/csv", "content": "id,name,amount\n"}))
        .await
        .expect("put header-only");

    // Empty file.
    let e = csvtab::table_from_blob(&state, &tenant, &json!({"doc": "empty.csv", "name": "e"})).await.expect_err("empty");
    assert_eq!(e.code(), "csv_empty", "{e:?}");
    let none = tables::table_schema(&state, &tenant, &json!({"table": "e"})).await.expect_err("no table for an empty file");
    assert_eq!(none.code(), "table_not_found");

    // Header-only file: refused, but the (empty) table exists.
    let e = csvtab::table_from_blob(&state, &tenant, &json!({"doc": "head.csv", "name": "h"})).await.expect_err("header only");
    assert_eq!(e.code(), "csv_no_rows", "{e:?}");
    let schema = tables::table_schema(&state, &tenant, &json!({"table": "h"})).await.expect("table h was created");
    assert!(schema.to_string().contains("amount"), "created with the header's columns: {schema}");
    let q = tables::table_query(&state, &tenant, &json!({"sql": "SELECT COUNT(*) AS n FROM h"}), None).await.expect("count");
    assert_eq!(q["rows"][0]["n"], json!(0), "0 rows");

    // Neither blob nor doc.
    let e = csvtab::table_from_blob(&state, &tenant, &json!({"name": "x"})).await.expect_err("no source");
    assert_eq!(e.code(), "args_invalid", "{e:?}");
    let msg = format!("{e:?}");
    assert!(msg.contains("blob") && msg.contains("doc"), "the refusal must name both arguments: {msg}");
    // Both at once is the same refusal.
    let e = csvtab::table_from_blob(&state, &tenant, &json!({"name": "x", "blob": "a", "doc": "b"})).await.expect_err("both");
    assert_eq!(e.code(), "args_invalid", "{e:?}");
    std::fs::remove_dir_all(&dir).ok();
}
