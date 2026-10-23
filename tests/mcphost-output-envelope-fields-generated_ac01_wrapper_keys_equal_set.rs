//! PRD-mcphost-output-envelope-fields-generated AC1 -- Given the six wrapper
//! paths, When each is driven with a stub tool returning `{"x": 1}`, Then every
//! added key beyond `x` and the enclosing tool's declared outputs is in
//! `OUTPUT_ENVELOPE_FIELDS`, and the union over the paths equals the set.

use mcphost::envelope::{OUTPUT_ENVELOPE_FIELDS, check_added_keys, wrapper_paths};
use serde_json::json;
use std::collections::BTreeSet;

pub fn declared_outputs(tool: &str) -> Vec<String> {
    mcphost::output_schemas::output_schema_for(tool)
        .and_then(|s| s.get("properties").and_then(|p| p.as_object()).map(|p| p.keys().cloned().collect()))
        .unwrap_or_default()
}

#[test]
fn mcphost_output_envelope_fields_generated_ac01() {
    let paths = wrapper_paths();
    assert_eq!(paths.len(), 6, "the AC 1 universe is six wrapper paths");
    let mut union = BTreeSet::new();
    for p in &paths {
        let out = (p.apply)(json!({"x": 1}));
        assert_eq!(out["x"], 1, "{}: the stub's own key survives", p.name);
        let added = check_added_keys(p.name, &declared_outputs(p.tool), &out).unwrap_or_else(|e| panic!("{e}"));
        union.extend(added);
    }
    let set: BTreeSet<String> = OUTPUT_ENVELOPE_FIELDS.iter().map(|s| s.to_string()).collect();
    assert_eq!(union, set, "union of keys added over the six paths must equal OUTPUT_ENVELOPE_FIELDS");
}
