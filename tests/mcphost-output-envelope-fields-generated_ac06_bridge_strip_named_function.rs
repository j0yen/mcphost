//! PRD-mcphost-output-envelope-fields-generated AC6 -- Given the bridge strip
//! extracted into a named function, When a value carrying `__resources_used`
//! and `x` is passed, Then the result lacks `__resources_used`, keeps `x`, and
//! the AC 1 rule still passes because the set is "may carry".

use mcphost::envelope::{OUTPUT_ENVELOPE_FIELDS, RESOURCES_USED_KEY, check_added_keys, strip_bridge_resources_used, wrapper_paths};
use serde_json::json;

#[test]
fn mcphost_output_envelope_fields_generated_ac06() {
    let out = strip_bridge_resources_used(json!({"x": 1, RESOURCES_USED_KEY: {"cpu_ms": 1}}));
    assert!(out.get(RESOURCES_USED_KEY).is_none(), "marker stripped: {out}");
    assert_eq!(out["x"], 1, "the callee's own key survives");

    // Non-objects pass through untouched.
    assert_eq!(strip_bridge_resources_used(json!([1])), json!([1]));

    // The strip removes a member no bridge result carries, yet the member stays
    // in the set because the other paths still add it.
    assert!(OUTPUT_ENVELOPE_FIELDS.contains(&RESOURCES_USED_KEY));
    for p in wrapper_paths() {
        check_added_keys(p.name, &[], &(p.apply)(json!({"x": 1}))).unwrap_or_else(|e| panic!("{e}"));
    }
}
