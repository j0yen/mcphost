//! PRD-mcphost-input-schema-is-the-validator
//! AC5 (P0) -- Given every tool returned by `host_tools(kinds, true)`, When a
//! call is built from its own schema with the test placeholder generator and
//! dispatched with a fixture tenant, Then no tool answers a structural
//! `args_invalid`, and the test fails naming the tool and the refusal text
//! otherwise.

use crate::common;
use common::{McpClient, TestServer, signup};
use mcphost::handler::host_tool_descriptors;
use mcphost::kinds::KindRegistry;
use serde_json::{Map, Value, json};
use std::time::Duration;

/// Strings tried, in order, for a string property; the first one the
/// property's own subschema accepts wins (so `pattern`, `enum` and
/// `minLength` are satisfied from the schema, never from a per-tool table).
const STRING_CANDIDATES: &[&str] = &["x", "ab", "a1", "abc_1", "A", "0a", "https://example.com/a", "@x", "2026-01-01"];

fn valid(schema: &Value, candidate: &Value) -> bool {
    jsonschema::validator_for(schema).map(|v| v.is_valid(candidate)).unwrap_or(false)
}

/// A value satisfying `schema`'s `enum`, `type`, `pattern`, `minimum`,
/// `minItems` and `propertyNames`, with `required` (and the first `anyOf`
/// branch) filled for objects.
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

fn fill_object(schema: &Value, obj: &mut Map<String, Value>) {
    let props = schema.get("properties").and_then(Value::as_object);
    let mut need: Vec<String> = schema
        .get("required")
        .and_then(Value::as_array)
        .map(|r| r.iter().filter_map(Value::as_str).map(str::to_string).collect())
        .unwrap_or_default();
    if let Some(branch) = schema.get("anyOf").and_then(Value::as_array).and_then(|b| b.first()) {
        need.extend(branch.get("required").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).map(str::to_string));
    }
    for cond in schema.get("allOf").and_then(Value::as_array).into_iter().flatten() {
        let applies = cond.get("if").is_some_and(|i| valid(i, &Value::Object(obj.clone())));
        if applies {
            need.extend(cond.pointer("/then/required").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).map(str::to_string));
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

/// The schema-derived call for a tool.
fn call_for(schema: &Map<String, Value>) -> Value {
    let schema = Value::Object(schema.clone());
    let mut obj = Map::new();
    fill_object(&schema, &mut obj);
    Value::Object(obj)
}

/// A refusal that a schema constraint (or a hand-written "missing required
/// argument" check) produced: the structural class, not a business error.
fn structural(err: &common::RpcError) -> bool {
    err.error_code.as_deref() == Some("args_invalid")
        && (err.data.get("constraint").is_some() || err.message.contains("missing required argument"))
}

#[test]
fn the_placeholder_generator_satisfies_pattern_enum_minimum_and_min_items() {
    let schema = json!({
        "type": "object",
        "required": ["name", "level", "n", "xs"],
        "properties": {
            "name": {"type": "string", "pattern": "^[a-z][a-z0-9_]{1,40}$"},
            "level": {"type": "string", "enum": ["info", "warn"]},
            "n": {"type": "integer", "minimum": 60, "maximum": 86400},
            "xs": {"type": "array", "items": {"type": "string"}, "minItems": 2, "maxItems": 6},
        },
        "anyOf": [{"required": ["name"]}],
    });
    let call = call_for(schema.as_object().expect("object"));
    assert!(valid(&schema, &call), "{call}");
}

#[tokio::test]
async fn no_host_tool_answers_a_structural_args_invalid_to_a_call_built_from_its_own_schema() {
    // One fixture tenant per tool, so the signup limit is lifted.
    let server = TestServer::start_with_signup_rate_limit(100_000).await;
    let tools = host_tool_descriptors(&KindRegistry::with_builtin());
    assert!(tools.len() >= 170, "only {} tools", tools.len());
    let mut offenders = Vec::new();
    for tool in &tools {
        let name = tool.name.to_string();
        let args = call_for(&tool.input_schema);
        // A fixture tenant per tool: key- and tenant-destroying verbs cannot
        // disturb the next call.
        let (_ns, key) = signup(&server.base_url, "Insch AC5").await;
        let client = McpClient::with_bearer(&server.base_url, &key);
        match tokio::time::timeout(Duration::from_secs(20), client.tools_call(&name, args.clone())).await {
            Ok(Err(err)) if structural(&err) => {
                offenders.push(format!("{name} {args}: {}", err.message));
            }
            _ => {}
        }
    }
    assert!(offenders.is_empty(), "structural args_invalid for a call built from the tool's own schema:\n{}", offenders.join("\n"));
}
