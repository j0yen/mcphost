//! PRD-mcphost-csv-to-table-one-call
//! AC4 -- Given `types: {zip: "text"}` on a column that would infer
//! `integer`, When loaded, Then `columns[zip].type` is `text` and `00123` is
//! stored verbatim.
//!
//! "Would infer integer": the sample (`infer_rows: 3`) sees only 94110,
//! 10001 and 60601, so without the override `zip` is `integer` and the
//! `00123` on line 5 is refused by name; with the override only that column
//! changes and the value is stored as written.

use crate::common;
use mcphost::{csvtab, docs, tables};
use serde_json::json;

const CSV: &str = "id,zip,city\n1,94110,SF\n2,10001,NYC\n3,60601,CHI\n4,00123,Odd\n";

fn scratch_dir(label: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("mcphost-csvtab-{label}-{}-{}", std::process::id(), mcphost::state::now_unix_ms()));
    std::fs::create_dir_all(&dir).expect("scratch dir");
    dir
}

fn col<'a>(out: &'a serde_json::Value, name: &str) -> &'a serde_json::Value {
    out["columns"].as_array().expect("columns").iter().find(|c| c["name"] == name).expect("column")
}

#[tokio::test]
async fn override_makes_zip_text_and_stores_00123_verbatim() {
    let dir = scratch_dir("ac04");
    let state = common::bare_state(&dir).await;

    // Baseline on its own tenant: the premise ("would infer integer") holds.
    let t0 = common::bare_tenant(&state, "csvtab-ac04-base").await;
    docs::doc_put(&state, &t0, &json!({"name": "z.csv", "mime": "text/csv", "content": CSV})).await.expect("put");
    let base = csvtab::table_from_blob(&state, &t0, &json!({"doc": "z.csv", "name": "places", "infer_rows": 3}))
        .await
        .expect("baseline load");
    assert_eq!(col(&base, "zip")["type"], json!("integer"), "without the override zip infers integer");
    assert_eq!(base["rows_skipped"], json!(1), "00123 is refused, not silently stored as 123");
    assert_eq!(base["skipped"][0]["column"], json!("zip"));

    // With the override.
    let t1 = common::bare_tenant(&state, "csvtab-ac04-over").await;
    docs::doc_put(&state, &t1, &json!({"name": "z.csv", "mime": "text/csv", "content": CSV})).await.expect("put");
    let out = csvtab::table_from_blob(
        &state,
        &t1,
        &json!({"doc": "z.csv", "name": "places", "infer_rows": 3, "types": {"zip": "text"}}),
    )
    .await
    .expect("override load");
    assert_eq!(col(&out, "zip")["type"], json!("text"));
    assert_eq!(col(&out, "id")["type"], json!("integer"), "only the named column's inference is overridden");
    assert_eq!(out["rows_loaded"], json!(4));
    assert_eq!(out["rows_skipped"], json!(0));

    let q = tables::table_query(&state, &t1, &json!({"sql": "SELECT zip FROM places WHERE id = 4"}), None).await.expect("query");
    assert_eq!(q["rows"][0]["zip"], json!("00123"), "stored verbatim");
    std::fs::remove_dir_all(&dir).ok();
}
