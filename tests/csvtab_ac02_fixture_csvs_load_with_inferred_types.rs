//! PRD-mcphost-csv-to-table-one-call
//! AC2 -- Given the 20 fixture CSVs in `tests/fixtures/csvtab/` (comma,
//! semicolon, tab, BOM, UTF-16, quoted newlines, empty trailing line,
//! 1,000-row real export), When each is loaded with defaults, Then >= 18
//! report `rows_skipped: 0` and every inferred `type` matches the fixture's
//! `expected.json`.
//!
//! The 1,000-row file is `examples/database-in-a-minute/fixture.csv` (the
//! recipe's own export). Fixture 20 is deliberately ragged (one row with an
//! extra field), so exactly 19 of 20 load clean.

use crate::common;
use mcphost::{csvtab, docs};
use serde_json::{Value, json};
use std::path::Path;

fn fixtures_dir() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/csvtab")
}

fn scratch_dir(label: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("mcphost-csvtab-{label}-{}-{}", std::process::id(), mcphost::state::now_unix_ms()));
    std::fs::create_dir_all(&dir).expect("scratch dir");
    dir
}

#[tokio::test]
async fn twenty_fixtures_load_with_the_expected_types_and_at_least_eighteen_skip_nothing() {
    use base64::Engine as _;
    let expected: Value =
        serde_json::from_str(&std::fs::read_to_string(fixtures_dir().join("expected.json")).expect("expected.json"))
            .expect("expected.json parses");
    let expected = expected.as_object().expect("object");
    assert_eq!(expected.len(), 20, "the fixture set is 20 files");

    let dir = scratch_dir("ac02");
    let state = common::bare_state(&dir).await;
    let mut clean = 0;
    for (i, (file, want)) in expected.iter().enumerate() {
        let bytes = std::fs::read(fixtures_dir().join(file)).expect("fixture file");
        let tenant = common::bare_tenant(&state, &format!("csvtab-ac02-{i:02}")).await;
        docs::doc_put(
            &state,
            &tenant,
            &json!({"name": file, "mime": "text/csv",
                    "content_base64": base64::engine::general_purpose::STANDARD.encode(&bytes)}),
        )
        .await
        .unwrap_or_else(|e| panic!("{file}: doc_put: {e:?}"));

        let out = csvtab::table_from_blob(&state, &tenant, &json!({"doc": file, "name": "t"}))
            .await
            .unwrap_or_else(|e| panic!("{file}: from_blob: {e:?}"));

        assert_eq!(out["rows_loaded"], want["rows"], "{file}: rows_loaded");
        assert_eq!(out["rows_skipped"], want["rows_skipped"], "{file}: rows_skipped");
        assert_eq!(out["delimiter"], want["delimiter"], "{file}: detected delimiter");
        let cols = out["columns"].as_array().expect("columns");
        let want_types = want["types"].as_object().expect("types");
        assert_eq!(cols.len(), want_types.len(), "{file}: column count {cols:?}");
        for c in cols {
            let name = c["name"].as_str().expect("name");
            assert_eq!(c["type"], want_types[name], "{file}: type of column {name}: {c}");
            if let Some(sources) = want.get("source_names") {
                assert_eq!(c["source_name"], sources[name], "{file}: source_name of {name}");
            }
        }
        if out["rows_skipped"] == json!(0) {
            clean += 1;
        }
        // The rows really landed and are queryable in SQL.
        let q = mcphost::tables::table_query(&state, &tenant, &json!({"sql": "SELECT COUNT(*) AS n FROM t"}), None)
            .await
            .unwrap_or_else(|e| panic!("{file}: query: {e:?}"));
        assert_eq!(q["rows"][0]["n"], want["rows"], "{file}: SELECT COUNT(*)");
    }
    println!("csvtab AC2: {clean}/20 fixtures loaded with rows_skipped: 0");
    assert!(clean >= 18, "only {clean}/20 fixtures loaded with rows_skipped: 0");
    std::fs::remove_dir_all(&dir).ok();
}
