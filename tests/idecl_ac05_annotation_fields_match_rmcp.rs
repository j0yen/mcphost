//! PRD-mcphost-tool-idempotent-declaration AC5 -- Given rmcp's
//! `ToolAnnotations`, When serialised, Then the keys equal
//! `ANNOTATION_FIELDS` exactly. The names come from the MCP library's serde
//! attributes, not from prose.

use mcphost::handler::{ANNOTATION_FIELDS, tool_annotations};
use rmcp::model::ToolAnnotations;
use serde_json::Value;

#[test]
fn serialised_annotation_keys_equal_annotation_fields() {
    let mut annotations = ToolAnnotations::default();
    annotations.idempotent_hint = Some(true);
    annotations.read_only_hint = Some(true);
    annotations.destructive_hint = Some(false);
    let Value::Object(map) = serde_json::to_value(&annotations).unwrap() else {
        panic!("ToolAnnotations serialises to an object");
    };
    let mut keys: Vec<&str> = map.keys().map(String::as_str).collect();
    keys.sort_unstable();
    let mut expected: Vec<&str> = ANNOTATION_FIELDS.to_vec();
    expected.sort_unstable();
    // `openWorldHint`/`title` are rmcp fields mcphost leaves unset (PRD open
    // question); with every field mcphost sets populated, no others appear.
    assert_eq!(keys, expected, "rmcp ToolAnnotations keys drifted from ANNOTATION_FIELDS");
}

#[test]
fn what_mcphost_emits_carries_exactly_those_keys() {
    for (idempotent, read_only) in [(true, true), (true, false), (false, false)] {
        let Value::Object(map) = serde_json::to_value(tool_annotations(idempotent, read_only)).unwrap() else {
            panic!("object");
        };
        let mut keys: Vec<&str> = map.keys().map(String::as_str).collect();
        keys.sort_unstable();
        let mut expected: Vec<&str> = ANNOTATION_FIELDS.to_vec();
        expected.sort_unstable();
        assert_eq!(keys, expected);
    }
}
