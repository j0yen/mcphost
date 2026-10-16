//! PRD-mcphost-csv-to-table-one-call
//! AC2 -- Given the 20 fixture CSVs in `tests/fixtures/csvtab/`, When each is
//! loaded with defaults, Then >= 18 report `rows_skipped: 0` and every
//! inferred `type` matches the fixture's `expected.json`.

use mcphost::csvtab::{Options, load};
use serde_json::Value;

#[test]
fn csvtab_ac02() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/csvtab");
    let expected: Value =
        serde_json::from_slice(&std::fs::read(dir.join("expected.json")).expect("expected.json")).expect("parse");
    let expected = expected.as_object().expect("object");
    assert_eq!(expected.len(), 20, "the corpus is 20 fixtures");

    let mut clean = 0;
    for (file, want) in expected {
        let bytes = std::fs::read(dir.join(file)).unwrap_or_else(|e| panic!("{file}: {e}"));
        let loaded = load(&bytes, &Options::default()).unwrap_or_else(|e| panic!("{file}: load failed: {e}"));
        let want_types = want["types"].as_object().expect("types");
        assert_eq!(loaded.columns.len(), want_types.len(), "{file}: column count");
        for col in &loaded.columns {
            assert_eq!(
                want_types.get(&col.name).and_then(Value::as_str),
                Some(col.ty.as_str()),
                "{file}: inferred type of column '{}'",
                col.name
            );
        }
        assert_eq!(loaded.rows.len() as u64, want["rows"].as_u64().unwrap(), "{file}: rows_loaded");
        assert_eq!(loaded.rows_skipped as u64, want["rows_skipped"].as_u64().unwrap(), "{file}: rows_skipped");
        if loaded.rows_skipped == 0 {
            clean += 1;
        }
    }
    assert!(clean >= 18, "only {clean}/20 fixtures loaded with rows_skipped 0");
}
