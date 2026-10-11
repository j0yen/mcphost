//! PRD-mcphost-csv-to-table-one-call
//! AC3 -- Given a CSV with `"n/a"` at line 412 of column `amount`, When
//! loaded with defaults, Then `rows_loaded` is total minus 1 and
//! `skipped[0]` is `{row: 412, column: "amount", reason: "not a real:
//! 'n/a'"}`; with `on_error: abort` no table exists afterwards.

use crate::common;
use mcphost::{csvtab, docs, tables};
use serde_json::{Value, json};

const TOTAL_ROWS: usize = 1_000;
const BAD_LINE: usize = 412;

/// Header on line 1, so data row `k` (1-based) sits on file line `k + 1`.
fn csv_with_bad_cell() -> String {
    let mut s = String::from("id,item,amount\n");
    for k in 1..=TOTAL_ROWS {
        let line = k + 1;
        let amount = if line == BAD_LINE { "n/a".to_string() } else { format!("{}.25", k * 3) };
        s.push_str(&format!("{k},item-{k},{amount}\n"));
    }
    s
}

fn scratch_dir(label: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("mcphost-csvtab-{label}-{}-{}", std::process::id(), mcphost::state::now_unix_ms()));
    std::fs::create_dir_all(&dir).expect("scratch dir");
    dir
}

#[tokio::test]
async fn skip_names_row_412_and_abort_creates_nothing() {
    let dir = scratch_dir("ac03");
    let state = common::bare_state(&dir).await;
    let tenant = common::bare_tenant(&state, "csvtab-ac03").await;
    docs::doc_put(&state, &tenant, &json!({"name": "sales.csv", "mime": "text/csv", "content": csv_with_bad_cell()}))
        .await
        .expect("put");

    // on_error: abort first -- the refusal names the cell and no table exists afterwards.
    let err = csvtab::table_from_blob(&state, &tenant, &json!({"doc": "sales.csv", "name": "sales", "on_error": "abort"}))
        .await
        .expect_err("abort refuses the load");
    assert_eq!(err.code(), "csv_row_invalid", "{err:?}");
    let data = format!("{err:?}");
    assert!(data.contains("412") && data.contains("amount"), "refusal must name row and column: {data}");
    let missing = tables::table_schema(&state, &tenant, &json!({"table": "sales"})).await.expect_err("no table after abort");
    assert_eq!(missing.code(), "table_not_found", "{missing:?}");

    // Defaults (on_error: skip): the one bad row is left out and named.
    let out = csvtab::table_from_blob(&state, &tenant, &json!({"doc": "sales.csv", "name": "sales"}))
        .await
        .expect("skip load");
    assert_eq!(out["rows_loaded"], json!(TOTAL_ROWS - 1));
    assert_eq!(out["rows_skipped"], json!(1));
    assert_eq!(
        out["skipped"][0],
        json!({"row": 412, "column": "amount", "reason": "not a real: 'n/a'"}),
        "skipped[0]"
    );
    let amount = out["columns"].as_array().expect("columns").iter().find(|c| c["name"] == "amount").expect("amount");
    assert_eq!(amount["type"], json!("real"), "one stray cell must not demote the column to text");

    let q = tables::table_query(&state, &tenant, &json!({"sql": "SELECT COUNT(*) AS n FROM sales"}), None)
        .await
        .expect("query");
    assert_eq!(q["rows"][0]["n"], Value::from(TOTAL_ROWS - 1));
    std::fs::remove_dir_all(&dir).ok();
}
