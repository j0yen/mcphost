//! PRD-mcphost-schema-coercion-opt-in AC7 -- Given every row of
//! `COERCIONS`, When its positive case is coerced, Then the result
//! round-trips to the original token, and its negative case is refused.
//! The case table and `COERCIONS` cannot drift: each must name the other's rows.

use mcphost::kinds::coerce::COERCIONS;
use serde_json::{Value, json};

/// (from, to, positives, negatives)
type Case = (&'static str, &'static str, Vec<Value>, Vec<Value>);

fn cases() -> Vec<Case> {
    vec![
        ("string", "integer", vec![json!("4"), json!("-7"), json!("0")], vec![json!("4.5"), json!(" 4"), json!("1e3"), json!("yes"), json!(""), json!("99999999999999999999")]),
        ("string", "number", vec![json!("4.5"), json!("-0.25"), json!("4")], vec![json!("1e3"), json!(" 4.5"), json!("NaN"), json!("inf"), json!("4.50")]),
        ("number", "integer", vec![json!(4.0), json!(-3.0)], vec![json!(4.78), json!(1e30), json!(0.2)]),
        ("string", "boolean", vec![json!("true"), json!("false")], vec![json!("yes"), json!("True"), json!("1")]),
        ("string", "null", vec![json!("null")], vec![json!("None"), json!("NULL"), json!("")]),
    ]
}

fn render(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

#[test]
fn case_table_and_coercions_name_the_same_rows() {
    let mut have: Vec<_> = COERCIONS.iter().map(|c| (c.from, c.to)).collect();
    let mut want: Vec<_> = cases().iter().map(|c| (c.0, c.1)).collect();
    have.sort();
    want.sort();
    assert_eq!(have, want);
}

#[test]
fn every_row_round_trips_positives_and_refuses_negatives() {
    for (from, to, positives, negatives) in cases() {
        let row = COERCIONS.iter().find(|c| c.from == from && c.to == to).expect("row");
        for p in positives {
            let out = (row.apply)(&p).unwrap_or_else(|| panic!("{from}→{to}: {p} must coerce"));
            if from == "number" {
                assert_eq!(out.as_f64(), p.as_f64(), "{p} → {out} must keep its value");
                assert!(out.is_i64());
            } else {
                assert_eq!(render(&out), render(&p), "{from}→{to}: {p} → {out} must print back as the token");
            }
        }
        for n in negatives {
            assert!((row.apply)(&n).is_none(), "{from}→{to}: {n} must be refused");
        }
    }
}
