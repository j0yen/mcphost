//! Deterministic example arguments generated from a tool's `args_schema`
//! (PRD-mcphost-tool-test-examples-from-schema).
//!
//! One pure function shared by `host.tool.test`, the `host.tool.publish`
//! dry run and `host.spec.test`, so the example cannot differ by call path.

use serde_json::{Map, Value, json};

/// Recursion stops at this depth; the cut path is listed under `untyped`.
pub const MAX_DEPTH: usize = 8;

/// A generated example plus the keys the generator had no type information for.
#[derive(Debug, Clone, PartialEq)]
pub struct Example {
    pub args: Value,
    pub untyped: Vec<String>,
}

/// The generated value does not satisfy the schema it came from.
#[derive(Debug, Clone, PartialEq)]
pub struct ExampleUnavailable {
    /// JSON pointer of the failing instance location, e.g. `/n`.
    pub path: String,
    pub message: String,
}

/// Generate one example for `schema` (no self-check; see [`checked_example`]).
pub fn example_from_schema(schema: &Value) -> Example {
    let mut untyped = Vec::new();
    let args = generate(schema, 0, "", &mut untyped).unwrap_or_else(|| json!({}));
    Example { args, untyped }
}

/// [`example_from_schema`] plus a self-check with the validator the call path uses.
pub fn checked_example(schema: &Value) -> Result<Example, ExampleUnavailable> {
    let ex = example_from_schema(schema);
    let validator = jsonschema::validator_for(schema).map_err(|e| ExampleUnavailable {
        path: String::new(),
        message: format!("args_schema is not a valid JSON schema: {e}"),
    })?;
    if let Err(e) = validator.validate(&ex.args) {
        return Err(ExampleUnavailable {
            path: e.instance_path.as_str().to_string(),
            message: e.to_string(),
        });
    }
    Ok(ex)
}

fn join(path: &str, key: &str) -> String {
    if path.is_empty() {
        key.to_string()
    } else {
        format!("{path}.{key}")
    }
}

fn first_type(schema: &Value) -> Option<&str> {
    match schema.get("type") {
        Some(Value::String(s)) => Some(s.as_str()),
        Some(Value::Array(list)) => list.first().and_then(Value::as_str),
        _ => {
            if schema.get("properties").is_some() {
                Some("object")
            } else if schema.get("items").is_some() {
                Some("array")
            } else {
                None
            }
        }
    }
}

/// `None` means "no type information" (the caller decides what to do).
fn generate(schema: &Value, depth: usize, path: &str, untyped: &mut Vec<String>) -> Option<Value> {
    if let Some(d) = schema.get("default") {
        return Some(d.clone());
    }
    if let Some(c) = schema.get("const") {
        return Some(c.clone());
    }
    if let Some(first) = schema.get("enum").and_then(Value::as_array).and_then(|a| a.first()) {
        return Some(first.clone());
    }
    let ty = first_type(schema)?;
    if depth >= MAX_DEPTH {
        untyped.push(if path.is_empty() { "$".to_string() } else { path.to_string() });
        return Some(match ty {
            "object" => json!({}),
            "array" => json!([]),
            "string" => json!("example"),
            "integer" => json!(1),
            "number" => json!(1.5),
            "boolean" => json!(true),
            _ => Value::Null,
        });
    }
    Some(match ty {
        "string" => json!(gen_string(schema)),
        "integer" => json!(gen_integer(schema)),
        "number" => gen_number(schema),
        "boolean" => json!(true),
        "null" => Value::Null,
        "array" => gen_array(schema, depth, path, untyped),
        "object" => gen_object(schema, depth, path, untyped),
        _ => return None,
    })
}

fn gen_string(schema: &Value) -> String {
    let mut s = match schema.get("format").and_then(Value::as_str) {
        Some("date") => "2026-01-01".to_string(),
        Some("date-time") => "2026-01-01T00:00:00Z".to_string(),
        Some("email") => "user@example.com".to_string(),
        Some("uri") | Some("url") => "https://example.com".to_string(),
        _ => "example".to_string(),
    };
    let min = schema.get("minLength").and_then(Value::as_u64).unwrap_or(0) as usize;
    while s.chars().count() < min {
        s.push('x');
    }
    if let Some(max) = schema.get("maxLength").and_then(Value::as_u64)
        && schema.get("format").is_none()
    {
        s = s.chars().take(max as usize).collect();
    }
    s
}

fn gen_integer(schema: &Value) -> i64 {
    let min = schema.get("minimum").and_then(Value::as_f64);
    let excl = schema.get("exclusiveMinimum").and_then(Value::as_f64);
    let max = schema.get("maximum").and_then(Value::as_f64);
    if let Some(m) = min {
        return m.ceil() as i64;
    }
    if let Some(m) = excl {
        return m.floor() as i64 + 1;
    }
    match max {
        Some(m) if m < 1.0 => m.floor() as i64,
        _ => 1,
    }
}

fn gen_number(schema: &Value) -> Value {
    let min = schema.get("minimum").and_then(Value::as_f64);
    let excl = schema.get("exclusiveMinimum").and_then(Value::as_f64);
    let max = schema.get("maximum").and_then(Value::as_f64);
    if let Some(m) = min {
        return json!(m);
    }
    if let Some(m) = excl {
        return json!(m + 0.5);
    }
    match max {
        Some(m) if m < 1.5 => json!(m),
        _ => json!(1.5),
    }
}

fn gen_array(schema: &Value, depth: usize, path: &str, untyped: &mut Vec<String>) -> Value {
    let n = schema.get("minItems").and_then(Value::as_u64).unwrap_or(0) as usize;
    let mut out = Vec::with_capacity(n);
    if n > 0 {
        let item_path = format!("{path}[0]");
        match schema.get("items").filter(|i| i.is_object()) {
            Some(items) => {
                for _ in 0..n {
                    // Each item's untyped keys repeat; record them once.
                    let mut local = Vec::new();
                    out.push(generate(items, depth + 1, &item_path, &mut local).unwrap_or_else(|| {
                        local.push(item_path.clone());
                        json!("example")
                    }));
                    for u in local {
                        if !untyped.contains(&u) {
                            untyped.push(u);
                        }
                    }
                }
            }
            None => {
                untyped.push(item_path);
                out.resize(n, json!("example"));
            }
        }
    }
    Value::Array(out)
}

fn gen_object(schema: &Value, depth: usize, path: &str, untyped: &mut Vec<String>) -> Value {
    let mut out = Map::new();
    let props = schema.get("properties").and_then(Value::as_object);
    let required: Vec<&str> = schema
        .get("required")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    // Required keys first (declared order), then optional keys in schema order.
    for key in &required {
        let p = join(path, key);
        let sub = props.and_then(|p| p.get(*key));
        let v = sub.and_then(|s| generate(s, depth + 1, &p, untyped));
        out.insert(
            (*key).to_string(),
            v.unwrap_or_else(|| {
                untyped.push(p);
                json!(key)
            }),
        );
    }
    if let Some(props) = props {
        for (key, sub) in props {
            if out.contains_key(key) {
                continue;
            }
            let wanted = sub.get("default").is_some() || sub.get("enum").is_some() || sub.get("const").is_some();
            if wanted {
                let p = join(path, key);
                if let Some(v) = generate(sub, depth + 1, &p, untyped) {
                    out.insert(key.clone(), v);
                }
            } else if first_type(sub).is_none() {
                untyped.push(join(path, key));
            }
        }
    }
    Value::Object(out)
}
