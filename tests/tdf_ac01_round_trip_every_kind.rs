//! PRD-mcphost-tool-directory-format
//! AC1 (P0) — Given one fixture payload per registered kind covering every
//! field in `known_spec_fields(kind)`, When `unpack` then `pack` runs, Then
//! every output equals its input byte-for-byte and no fixture needed a field
//! outside `tool.toml`, `tool.py`, `tool.wasm`, `tests/`.

use crate::common;
use mcphost::kinds::production_registry;
use mcphost::tooldir::{pack, unpack};
use serde_json::{Value, json};

/// A sample value per spec field name; the round trip is shape-only, so a
/// sample need not satisfy the kind's validator. Nested objects, arrays of
/// tables, floats, empty arrays and multi-line strings are all exercised.
fn sample(kind: &str, field: &str) -> Value {
    match field {
        "source" if kind == "python" => json!("import json\n\n\ndef run(args):\n    return {\"echo\": args}\n"),
        "source" => json!("export default (a) => a;\n"),
        "component" => json!(base64_of(b"\0asm\x0d\0\x01\0 component bytes \xff\xfe")),
        "args_schema" | "schema" => json!({
            "type": "object",
            "properties": {"msg": {"type": "string"}, "n": {"type": "integer", "minimum": 0}},
            "required": [],
            "additionalProperties": false,
        }),
        "steps" => json!([
            {"tool": "first", "input": {"x": "$.input.x"}},
            {"tool": "second", "input": {"y": "$.prev.y", "list": [1, 2, 3]}},
        ]),
        "headers" | "query" | "env" => json!({"a": "1", "b": "two"}),
        "body" | "response" | "outputs" => json!({"k": {"nested": [{"x": 1}, {"x": 2}]}, "f": 1.5}),
        "timeout_s" | "memory_mb" | "max_output_bytes" => json!(30),
        "network" | "requirements" | "secrets" => json!(["one", "two"]),
        "method" => json!("POST"),
        "program" => json!(".[] | select(.ok)\n"),
        "description" => json!("A tool.\nSecond line."),
        _ => json!("value"),
    }
}

fn base64_of(bytes: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

#[test]
fn tdf_ac01_unpack_then_pack_is_byte_identical_for_every_kind() {
    let registry = production_registry();
    let names = registry.names();
    assert!(names.len() >= 5, "expected the production kinds, got {names:?}");
    for kind_name in names {
        let kind = registry.get(kind_name).expect("registered kind");
        let mut spec = serde_json::Map::new();
        for field in kind.known_spec_fields() {
            spec.insert((*field).to_string(), sample(kind_name, field));
        }
        let input = json!({"name": format!("fx_{kind_name}"), "kind": kind_name, "spec": Value::Object(spec)});
        let scratch = common::TempDataDir::new();
        let dir = scratch.0.join("tool");
        unpack(&input, &dir, &registry).unwrap_or_else(|e| panic!("{kind_name}: unpack failed: {}", e.message));

        let mut entries: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        entries.sort();
        for entry in &entries {
            assert!(
                ["tool.toml", "tool.py", "tool.wasm", "tests"].contains(&entry.as_str()),
                "{kind_name}: fixture needed a file outside the format: {entry}"
            );
        }

        let output = pack(&dir).unwrap_or_else(|e| panic!("{kind_name}: pack failed: {}", e.message));
        assert_eq!(
            serde_json::to_string(&output).unwrap(),
            serde_json::to_string(&input).unwrap(),
            "{kind_name}: pack(unpack(p)) differs from p"
        );
    }
}
