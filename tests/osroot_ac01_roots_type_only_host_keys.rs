//! contract: reads registry handler::host_tool_descriptors
//! contract: reads contracts/host-output-schemas.v1.json
//! PRD-mcphost-tool-call-output-root-opaque
//! AC1 (P0) -- Given the composed registry, When the osroot root test reads
//! `host.tool.call` and `host.tool.test`, Then each root's typed properties are
//! exactly the envelope const plus `forwarded_to` and `client_tool`, the root
//! accepts additional properties of any type, `required` is `[]` for
//! `host.tool.call` and `["dry_run"]` for `host.tool.test`, and
//! `contracts/host-output-schemas.v1.json` matches the composed root, and
//! `data` is `{}`.

use mcphost::output_schemas::{FORWARDING_ROOTS, ROOT_ENVELOPE, live_required, output_schema_for};
use serde_json::{Map, Value, json};
use std::collections::BTreeSet;

fn committed() -> Map<String, Value> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("contracts/host-output-schemas.v1.json");
    serde_json::from_slice(&std::fs::read(path).expect("read contract")).expect("parse contract")
}

fn strip_null(node: &Value) -> Value {
    let mut node = node.clone();
    if let Some(types) = node.get("type").and_then(Value::as_array).cloned() {
        let rest: Vec<Value> = types.into_iter().filter(|t| t != "null").collect();
        node["type"] = if rest.len() == 1 { rest[0].clone() } else { Value::Array(rest) };
    }
    node
}

#[test]
fn osroot_ac01_roots_type_exactly_the_envelope_and_the_two_forward_keys() {
    assert_eq!(FORWARDING_ROOTS, ["host.tool.call", "host.tool.test"]);
    let committed = committed();
    for tool in FORWARDING_ROOTS {
        let schema = output_schema_for(tool).unwrap_or_else(|| panic!("{tool} registered"));
        let props = schema["properties"].as_object().expect("properties");
        let got: BTreeSet<&str> = props.keys().map(String::as_str).collect();
        let want: BTreeSet<&str> = ROOT_ENVELOPE.iter().copied().chain(["forwarded_to", "client_tool"]).collect();
        assert_eq!(got, want, "{tool}: typed root properties");

        assert_eq!(schema["type"], json!("object"), "{tool}");
        assert!(
            schema.get("additionalProperties").is_none_or(|a| a != &json!(false)),
            "{tool}: the root must accept additional properties"
        );
        assert_eq!(props["data"], json!({}), "{tool}: data is opaque");
        assert_eq!(props["result"], json!({}), "{tool}: result is opaque");

        let want_required: &[&str] = if *tool == "host.tool.test" { &["dry_run"] } else { &[] };
        assert_eq!(live_required(&schema), want_required, "{tool}: required");

        let on_disk = committed[*tool].as_object().expect("committed schema");
        // The loader makes every optional property nullable; the committed file
        // holds the type without that.
        let composed: Map<String, Value> = props.iter().map(|(k, v)| (k.clone(), strip_null(v))).collect();
        assert_eq!(on_disk["properties"], Value::Object(composed), "{tool}: committed contract vs composed root");
        assert_eq!(on_disk.get("required"), schema.get("required"), "{tool}: committed required");
        assert_eq!(on_disk["type"], schema["type"], "{tool}");
    }
}

#[test]
fn osroot_ac01_root_accepts_undeclared_keys_of_any_type() {
    for tool in FORWARDING_ROOTS {
        let schema = Value::Object((*output_schema_for(tool).expect("registered")).clone());
        let validator = jsonschema::validator_for(&schema).expect("compiles");
        let row = json!({"dry_run": true, "zz_a": 1, "zz_b": "s", "zz_c": null, "zz_d": {"x": [1]}, "checked_at": "2026-10-22"});
        validator.validate(&row).unwrap_or_else(|e| panic!("{tool}: {e}"));
    }
}
