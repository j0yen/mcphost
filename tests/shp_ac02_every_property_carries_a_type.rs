//! PRD-mcphost-schema-shape-agreement
//! contract: reads registry handler::host_tool_descriptors
//! AC2 (P0) -- Given every descriptor's input schema, When the generated scan walks
//! every property (through `properties`, `items`, `anyOf`, `oneOf`), Then each
//! property or each `anyOf`/`oneOf` branch carries `type`, `enum`, `const` or `$ref`,
//! else it fails naming tool and JSON pointer.

use crate::vbd_calls;
use serde_json::Value;

const TYPED: &[&str] = &["type", "enum", "const", "$ref"];

fn typed(schema: &Value) -> bool {
    TYPED.iter().any(|k| schema.get(*k).is_some())
}

/// A subschema is typed itself, or every branch of its `anyOf`/`oneOf` is.
fn carries_type(schema: &Value) -> bool {
    if typed(schema) {
        return true;
    }
    ["anyOf", "oneOf"].iter().any(|kw| {
        schema.get(*kw).and_then(Value::as_array).is_some_and(|branches| !branches.is_empty() && branches.iter().all(carries_type))
    })
}

/// Collects `tool pointer` for every untyped property under `schema`.
fn scan(tool: &str, schema: &Value, pointer: &str, out: &mut Vec<String>, visited: &mut usize) {
    if let Some(props) = schema.get("properties").and_then(Value::as_object) {
        for (name, sub) in props {
            *visited += 1;
            let at = format!("{pointer}/properties/{name}");
            if !carries_type(sub) {
                out.push(format!("{tool} {at}"));
            }
            scan(tool, sub, &at, out, visited);
        }
    }
    if let Some(items) = schema.get("items").filter(|i| i.is_object()) {
        let at = format!("{pointer}/items");
        if !carries_type(items) {
            out.push(format!("{tool} {at}"));
        }
        scan(tool, items, &at, out, visited);
    }
    for kw in ["anyOf", "oneOf"] {
        for (i, branch) in schema.get(kw).and_then(Value::as_array).into_iter().flatten().enumerate() {
            scan(tool, branch, &format!("{pointer}/{kw}/{i}"), out, visited);
        }
    }
}

#[test]
fn shp_ac02_every_property_or_each_of_its_branches_carries_a_type() {
    let mut untyped = Vec::new();
    let mut visited = 0;
    let tools = vbd_calls::all_descriptors();
    for tool in &tools {
        scan(&tool.name, &Value::Object((*tool.input_schema).clone()), "", &mut untyped, &mut visited);
    }
    println!("shp_ac02: {} descriptors, {visited} properties, {} untyped", tools.len(), untyped.len());
    assert!(tools.len() >= 250 && visited >= 500, "scan visited only {visited} properties of {} descriptors", tools.len());
    assert!(untyped.is_empty(), "{} properties carry no type/enum/const/$ref (nor typed anyOf/oneOf branches):\n{}", untyped.len(), untyped.join("\n"));
}

#[test]
fn shp_ac02_scan_names_an_untyped_property_and_accepts_typed_branches() {
    let schema = serde_json::json!({"properties": {
        "bad": {"description": "no type"},
        "ok": {"type": "string"},
        "split": {"oneOf": [{"type": "string"}, {"type": "array", "items": {"description": "untyped item"}}]},
        "half": {"anyOf": [{"type": "string"}, {"description": "untyped branch"}]},
    }});
    let (mut out, mut n) = (Vec::new(), 0);
    scan("t", &schema, "", &mut out, &mut n);
    assert!(out.contains(&"t /properties/bad".to_string()));
    assert!(out.contains(&"t /properties/half".to_string()));
    assert!(out.contains(&"t /properties/split/oneOf/1/items".to_string()));
    assert!(!out.contains(&"t /properties/ok".to_string()) && !out.contains(&"t /properties/split".to_string()));
}
