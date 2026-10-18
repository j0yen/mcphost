//! PRD-mcphost-input-schema-is-the-validator
//! AC3 (P0) -- Given host.index.create, host.task.post, host.ask.create,
//! host.table.append, host.policy.attrs_set, host.table.create and
//! host.tool.publish, When their schemas are read, Then `name` carries the
//! handler's `pattern`, `deadline_s` carries `minimum` 60 and `maximum`
//! 86400, `options` `minItems` 2 and `maxItems` 6, `sources` `minItems` 1 and
//! `maxItems` 20, `rows` is typed object or array, `attrs` object.

use mcphost::handler::host_tool_descriptors;
use mcphost::kinds::KindRegistry;
use serde_json::{Value, json};

fn property(tool: &str, arg: &str) -> Value {
    let tools = host_tool_descriptors(&KindRegistry::with_builtin());
    let tool = tools.iter().find(|t| t.name.as_ref() == tool).unwrap_or_else(|| panic!("{tool} not listed"));
    tool.input_schema["properties"][arg].clone()
}

/// Does `pattern` (as a JSON-schema keyword) accept `s`?
fn matches(pattern: &str, s: &str) -> bool {
    let v = jsonschema::validator_for(&json!({"type": "string", "pattern": pattern})).expect("pattern compiles");
    v.is_valid(&json!(s))
}

#[test]
fn index_create_name_pattern_and_sources_bounds() {
    let name = property("host.index.create", "name");
    assert_eq!(name["pattern"], mcphost::index::NAME_PATTERN);
    let pattern = name["pattern"].as_str().expect("pattern");
    for ok in ["a", "0docs", "my-index_1", &"a".repeat(64)] {
        assert!(matches(pattern, ok), "{ok} should match");
    }
    for bad in ["", "Docs", "-a", "_a", "a b", &"a".repeat(65)] {
        assert!(!matches(pattern, bad), "{bad} should not match");
    }
    let sources = property("host.index.create", "sources");
    assert_eq!(sources["minItems"], 1);
    assert_eq!(sources["maxItems"], 20);
    assert_eq!(sources["maxItems"], mcphost::index::SOURCES_MAX);
}

#[test]
fn task_post_deadline_bounds() {
    let d = property("host.task.post", "deadline_s");
    assert_eq!(d["minimum"], 60);
    assert_eq!(d["maximum"], 86400);
    assert_eq!(d["minimum"], mcphost::tasks::DEADLINE_S_MIN);
    assert_eq!(d["maximum"], mcphost::tasks::DEADLINE_S_MAX);
}

#[test]
fn ask_create_options_bounds() {
    let o = property("host.ask.create", "options");
    assert_eq!(o["minItems"], 2);
    assert_eq!(o["maxItems"], 6);
    assert_eq!(o["minItems"], mcphost::asks::OPTIONS_MIN);
    assert_eq!(o["maxItems"], mcphost::asks::OPTIONS_MAX);
}

#[test]
fn table_append_rows_is_object_or_array() {
    let rows = property("host.table.append", "rows");
    assert_eq!(rows["type"], json!(["object", "array"]));
}

#[test]
fn policy_attrs_set_attrs_is_object() {
    assert_eq!(property("host.policy.set", "rule").get("type"), None, "rule is untouched");
    assert_eq!(property("host.policy.attrs_set", "attrs")["type"], "object");
}

#[test]
fn table_create_name_pattern() {
    let name = property("host.table.create", "name");
    assert_eq!(name["pattern"], mcphost::tables::IDENT_PATTERN);
    let pattern = name["pattern"].as_str().expect("pattern");
    for ok in ["t", "_t", "Orders_2", &"a".repeat(64)] {
        assert!(matches(pattern, ok), "{ok} should match");
    }
    for bad in ["", "1a", "a-b", "a b", "a\"b", &"a".repeat(65)] {
        assert!(!matches(pattern, bad), "{bad} should not match");
    }
}

#[test]
fn tool_publish_name_pattern_agrees_with_the_validator() {
    let name = property("host.tool.publish", "name");
    assert_eq!(name["pattern"], mcphost::drafter::TOOL_NAME_REGEX);
    let pattern = name["pattern"].as_str().expect("pattern");
    for probe in ["ab", "a", "A1", "a_b9", "1ab", "a-b", &"a".repeat(41), &"a".repeat(42), ""] {
        assert_eq!(
            matches(pattern, probe),
            mcphost::state::validate_tool_name(probe).is_ok(),
            "pattern and validate_tool_name disagree on {probe:?}"
        );
    }
}
