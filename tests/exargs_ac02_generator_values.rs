//! PRD-mcphost-tool-test-examples-from-schema AC2 — a schema with integer
//! (with and without `minimum: 3`), number, enum, `format: date`, a default,
//! a required key and an optional untyped key generates `1`/`3`, `1.5`,
//! `"a"`, `"2026-01-01"`, `7`, with `q` present, the untyped key absent and
//! listed in `untyped`, and the integer never serialized as a float.

use mcphost::kinds::example::{checked_example, example_from_schema};
use serde_json::json;

fn schema() -> serde_json::Value {
    json!({
        "type": "object",
        "required": ["q", "i", "j", "x", "e", "d"],
        "properties": {
            "q": {},
            "i": {"type": "integer"},
            "j": {"type": "integer", "minimum": 3},
            "x": {"type": "number"},
            "e": {"enum": ["a", "b"]},
            "d": {"type": "string", "format": "date"},
            "k": {"type": "integer", "default": 7},
            "free": {}
        }
    })
}

#[test]
fn values_follow_the_schema_and_untyped_keys_are_listed() {
    let ex = example_from_schema(&schema());
    assert_eq!(ex.args["i"], json!(1));
    assert_eq!(ex.args["j"], json!(3));
    assert_eq!(ex.args["x"], json!(1.5));
    assert_eq!(ex.args["e"], json!("a"));
    assert_eq!(ex.args["d"], json!("2026-01-01"));
    assert_eq!(ex.args["k"], json!(7));
    assert!(ex.args.get("q").is_some(), "required key is present");
    assert!(ex.args.get("free").is_none(), "optional untyped key is absent");
    assert!(ex.untyped.contains(&"free".to_string()));
    assert!(ex.untyped.contains(&"q".to_string()));
}

#[test]
fn integers_serialize_without_a_decimal_point() {
    let ex = example_from_schema(&schema());
    let s = serde_json::to_string(&ex.args).unwrap();
    assert!(s.contains("\"i\":1,") || s.contains("\"i\":1}"), "{s}");
    assert!(s.contains("\"j\":3"), "{s}");
    assert!(!s.contains("\"i\":1.0") && !s.contains("\"j\":3.0"), "{s}");
}

#[test]
fn generated_example_passes_its_own_schema() {
    assert!(checked_example(&schema()).is_ok());
}
