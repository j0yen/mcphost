//! Shared helpers for the `vbd_ac*` proofs (PRD-mcphost-host-args-validated-by-default):
//! the schema-driven placeholder call generator (the `insch_ac05` rules: the
//! first `enum` member, a candidate string the property's own subschema
//! accepts, `required` plus the first `anyOf` branch and any applicable
//! `allOf/if/then` filled) and the descriptor set both ACs walk.
//!
//! `allow(dead_code)`: each including test file uses only part of this.
#![allow(dead_code)]

use mcphost::handler::{admin_tool_descriptors, host_tool_descriptors};
use mcphost::kinds::KindRegistry;
use rmcp::model::Tool;
use serde_json::{Map, Value, json};

const STRING_CANDIDATES: &[&str] = &["x", "ab", "a1", "abc_1", "A", "0a", "https://example.com/a", "@x", "2026-01-01"];

fn valid(schema: &Value, candidate: &Value) -> bool {
    jsonschema::validator_for(schema).map(|v| v.is_valid(candidate)).unwrap_or(false)
}

fn placeholder(schema: &Value) -> Value {
    if let Some(first) = schema.get("enum").and_then(Value::as_array).and_then(|e| e.first()) {
        return first.clone();
    }
    let ty = match schema.get("type") {
        Some(Value::String(t)) => t.clone(),
        Some(Value::Array(ts)) => ts.first().and_then(Value::as_str).unwrap_or("string").to_string(),
        _ => "string".to_string(),
    };
    match ty.as_str() {
        "integer" | "number" => {
            let min = schema.get("minimum").and_then(Value::as_i64);
            let max = schema.get("maximum").and_then(Value::as_i64);
            json!(min.or(max).unwrap_or(1))
        }
        "boolean" => json!(true),
        "array" => {
            let n = schema.get("minItems").and_then(Value::as_u64).unwrap_or(0) as usize;
            let item = schema.get("items").map(placeholder).unwrap_or_else(|| json!("x"));
            Value::Array(vec![item; n])
        }
        "object" => {
            let mut obj = Map::new();
            if let Some(kinds) = schema.pointer("/propertyNames/enum").and_then(Value::as_array)
                && let Some(k) = kinds.first().and_then(Value::as_str)
            {
                obj.insert(k.to_string(), json!("x"));
            }
            fill_object(schema, &mut obj);
            Value::Object(obj)
        }
        _ => STRING_CANDIDATES
            .iter()
            .map(|c| json!(c))
            .find(|c| valid(schema, c))
            .unwrap_or_else(|| json!("x")),
    }
}

fn names(v: Option<&Value>) -> Vec<String> {
    v.and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).map(str::to_string).collect()
}

fn fill_object(schema: &Value, obj: &mut Map<String, Value>) {
    let props = schema.get("properties").and_then(Value::as_object);
    let mut need = names(schema.get("required"));
    if let Some(branch) = schema.get("anyOf").and_then(Value::as_array).and_then(|b| b.first()) {
        need.extend(names(branch.get("required")));
    }
    for cond in schema.get("allOf").and_then(Value::as_array).into_iter().flatten() {
        if cond.get("if").is_some_and(|i| valid(i, &Value::Object(obj.clone()))) {
            need.extend(names(cond.pointer("/then/required")));
        }
    }
    for name in need {
        if obj.contains_key(&name) || name == "tenant_key" {
            continue;
        }
        let sub = props.and_then(|p| p.get(&name)).cloned().unwrap_or_else(|| json!({}));
        obj.insert(name, placeholder(&sub));
    }
}

/// The schema-derived call for a tool's `inputSchema`.
pub fn call_for(schema: &Map<String, Value>) -> Value {
    let schema = Value::Object(schema.clone());
    let mut obj = Map::new();
    fill_object(&schema, &mut obj);
    Value::Object(obj)
}

/// Every descriptor the control plane publishes to a keyed tenant, then the
/// admin ones.
pub fn all_descriptors() -> Vec<Tool> {
    host_tool_descriptors(&KindRegistry::with_builtin()).into_iter().chain(admin_tool_descriptors()).collect()
}

pub fn is_admin(tool: &Tool) -> bool {
    tool.name.starts_with("admin.")
}

/// A cap on a generated array/string so a huge declared bound stays a cheap call.
const BOUNDARY_CAP: u64 = 4096;

/// PRD-mcphost-schema-bound-agreement R2: the valid edges of one property's
/// subschema -- the empty array or string, `0`, each declared
/// `minItems`/`minLength`/`minimum`, each declared `maxItems`/`maxLength`/
/// `maximum`, and a max-length multibyte string. Invalid edges (max + 1) are
/// validator refusals by construction and are not generated.
pub fn edge_values(schema: &Value) -> Vec<(String, Value)> {
    let mut out: Vec<(String, Value)> = Vec::new();
    let ty = match schema.get("type") {
        Some(Value::String(t)) => t.as_str(),
        _ => "",
    };
    let item = schema.get("items").map(placeholder).unwrap_or_else(|| json!("x"));
    match ty {
        "array" => {
            out.push(("empty array".into(), json!([])));
            if let Some(n) = schema.get("minItems").and_then(Value::as_u64) {
                out.push(("minItems".into(), Value::Array(vec![item.clone(); n.min(BOUNDARY_CAP) as usize])));
            }
            if let Some(n) = schema.get("maxItems").and_then(Value::as_u64) {
                out.push(("maxItems".into(), Value::Array(vec![item; n.min(BOUNDARY_CAP) as usize])));
            }
        }
        "string" if schema.get("enum").is_none() => {
            out.push(("empty string".into(), json!("")));
            if let Some(n) = schema.get("minLength").and_then(Value::as_u64) {
                out.push(("minLength".into(), json!("a".repeat(n.min(BOUNDARY_CAP) as usize))));
            }
            if let Some(n) = schema.get("maxLength").and_then(Value::as_u64) {
                out.push(("maxLength".into(), json!("a".repeat(n.min(BOUNDARY_CAP) as usize))));
                out.push(("maxLength multibyte".into(), json!("\u{e9}".repeat(n.min(BOUNDARY_CAP) as usize))));
            }
        }
        "integer" | "number" => {
            out.push(("zero".into(), json!(0)));
            for key in ["minimum", "maximum"] {
                if let Some(n) = schema.get(key).and_then(Value::as_i64) {
                    out.push((key.into(), json!(n)));
                }
            }
        }
        _ => {}
    }
    out
}

/// Every schema-derived boundary call for a tool: the placeholder call with one
/// top-level property replaced by one of its [`edge_values`]. Returns
/// `(JSON pointer, edge label, args)`; the caller asks the validator whether it accepts each.
pub fn boundary_calls(schema: &Map<String, Value>) -> Vec<(String, String, Value)> {
    let base = call_for(schema);
    let Some(props) = schema.get("properties").and_then(Value::as_object) else { return Vec::new() };
    let mut out = Vec::new();
    for (name, sub) in props {
        if name == "tenant_key" {
            continue;
        }
        for (label, value) in edge_values(sub) {
            let mut args = base.clone();
            args[name.as_str()] = value;
            out.push((format!("/{name}"), label, args));
        }
    }
    out
}
