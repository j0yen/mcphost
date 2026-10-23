//! PRD-mcphost-output-envelope-fields-generated AC4 -- Given a test-only
//! wrapper fixture that inserts an extra key not in the set, When the AC 1 test
//! logic runs against it, Then it fails and the message names the extra key
//! and the wrapper path.

use mcphost::envelope::{WrapperPath, check_added_keys, wrapper_paths};
use serde_json::{Value, json};

fn rogue_wrapper(mut v: Value) -> Value {
    v["sneaky_meta"] = json!(true);
    v
}

#[test]
fn mcphost_output_envelope_fields_generated_ac04() {
    let rogue = WrapperPath { name: "rogue_wrapper", tool: "host.tool.call", apply: rogue_wrapper };
    let out = (rogue.apply)(json!({"x": 1}));
    let err = check_added_keys(rogue.name, &[], &out).expect_err("an extra key must fail the AC 1 rule");
    assert!(err.contains("sneaky_meta"), "message names the extra key: {err}");
    assert!(err.contains("rogue_wrapper"), "message names the wrapper path: {err}");

    // The real paths pass the same logic.
    for p in wrapper_paths() {
        check_added_keys(p.name, &[], &(p.apply)(json!({"x": 1}))).unwrap_or_else(|e| panic!("{e}"));
    }
}
