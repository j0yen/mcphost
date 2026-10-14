//! PRD-mcphost-tool-test-examples-from-schema AC6 — a schema nested 9 levels
//! deep stops at depth 8, names the cut path under `untyped`, and never panics.

use mcphost::kinds::example::{MAX_DEPTH, example_from_schema};
use serde_json::{Value, json};

/// object -> `c` array(minItems 1) -> object -> ... `levels` containers deep.
fn nested(levels: usize) -> Value {
    let mut schema = json!({"type": "string"});
    for i in 0..levels {
        schema = if i % 2 == 0 {
            json!({"type": "object", "required": ["c"], "properties": {"c": schema}})
        } else {
            json!({"type": "array", "minItems": 1, "items": schema})
        };
    }
    schema
}

#[test]
fn nine_levels_deep_stops_at_depth_eight_and_names_the_cut() {
    assert_eq!(MAX_DEPTH, 8);
    let ex = example_from_schema(&nested(9));
    assert!(!ex.untyped.is_empty(), "the cut path is listed: {:?}", ex.untyped);
    assert!(ex.untyped.iter().any(|p| p.starts_with("c[0].c[0]")), "{:?}", ex.untyped);
    // The value is finite and shallower than the schema.
    let depth = |v: &Value| -> usize {
        fn d(v: &Value) -> usize {
            match v {
                Value::Object(m) => 1 + m.values().map(d).max().unwrap_or(0),
                Value::Array(a) => 1 + a.iter().map(d).max().unwrap_or(0),
                _ => 0,
            }
        }
        d(v)
    };
    assert!(depth(&ex.args) <= 9, "{}", ex.args);
}

#[test]
fn shallow_schemas_report_no_cut() {
    assert!(example_from_schema(&nested(3)).untyped.is_empty());
}

#[test]
fn absurdly_deep_schema_does_not_panic() {
    let ex = example_from_schema(&nested(200));
    assert!(!ex.untyped.is_empty());
}
