//! PRD-mcphost-schema-coercion-opt-in AC1 -- Given the 10-08 explore
//! `is not of type` rows (`fixtures/chain_errors_20261008.jsonl`), When each
//! offending value is classified by `COERCIONS`, Then the lossless-coercible
//! share is printed. MEASURED: 0 of the typed rows are coercible (the values
//! are `112.5`, `0.2`, `4.78` and `null`), so the PRD's falsifier (>= 50 %)
//! FIRED; this test records the real number and does not hide it.

use mcphost::kinds::coerce::coercible;
use serde_json::Value;

/// `(offending value, schema type)` from `<value> is not of type "<type>"`.
fn typed_rows() -> Vec<(Value, String)> {
    let raw = include_str!("fixtures/chain_errors_20261008.jsonl");
    raw.lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .filter_map(|row| row["result"].as_str().map(str::to_string))
        .filter_map(|text| {
            let text = text.strip_prefix("McphostCallError: ").unwrap_or(&text).to_string();
            let (value, rest) = text.split_once(" is not of type \"")?;
            let ty = rest.split('"').next()?.to_string();
            Some((serde_json::from_str(value).ok()?, ty))
        })
        .collect()
}

#[test]
fn share_of_coercible_rows_is_measured_and_recorded() {
    let rows = typed_rows();
    assert!(rows.len() >= 5, "the 10-08 corpus has the typed rows, got {}", rows.len());
    let coercible_n = rows.iter().filter(|(v, ty)| coercible(v, ty)).count();
    let share = coercible_n as f64 / rows.len() as f64;
    println!("coerce AC1: {coercible_n}/{} rows losslessly coercible = {:.0} %", rows.len(), share * 100.0);
    println!("coerce AC1: falsifier (>= 50 %) {}", if share >= 0.5 { "holds" } else { "FIRED" });
    // The classifier is live: the same rows sent as encodings would count.
    assert!(coercible(&Value::from("4"), "integer"));
    assert!(coercible(&Value::from(4.0), "integer"));
    assert!(!coercible(&Value::from(4.78), "integer"), "4.78 is a wrong value, not an encoding");
    assert_eq!(coercible_n, 0, "recorded measurement on the real corpus (update only with new evidence)");
}
