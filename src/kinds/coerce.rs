//! PRD-mcphost-schema-coercion-opt-in: author-declared, lossless argument
//! coercion. A property that declares `"x-coerce": true` (an extension
//! keyword jsonschema ignores) accepts `"4"` and `4.0` for an `integer`;
//! everything else stays strict. [`check_args`] is the one gate every
//! schema-validation call site runs, so a call site cannot forget it.
//! Only the [`COERCIONS`] rows exist -- nothing rounds, trims or guesses.

use serde::Serialize;
use serde_json::{Number, Value};

use super::{ArgsError, describe_args_error, json_type_name};

/// One lossless coercion: a `from` JSON type (the vocabulary of
/// `json_type_name`) turned into a `to` schema type, or `None` (refused).
pub struct Coercion {
    pub from: &'static str,
    pub to: &'static str,
    pub apply: fn(&Value) -> Option<Value>,
}

/// Every coercion the host performs; nothing outside this table is ever
/// attempted. `tests/coerce_ac07_*` asserts each row round-trips.
pub static COERCIONS: &[Coercion] = &[
    Coercion { from: "string", to: "integer", apply: string_to_integer },
    Coercion { from: "string", to: "number", apply: string_to_number },
    Coercion { from: "number", to: "integer", apply: number_to_integer },
    Coercion { from: "string", to: "boolean", apply: string_to_boolean },
    Coercion { from: "string", to: "null", apply: string_to_null },
];

fn string_to_integer(v: &Value) -> Option<Value> {
    let s = v.as_str()?;
    let digits = s.strip_prefix(['+', '-']).unwrap_or(s);
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    s.parse::<i64>().ok().map(Value::from)
}

fn string_to_number(v: &Value) -> Option<Value> {
    let s = v.as_str()?;
    if let Some(i) = string_to_integer(v) {
        return Some(i);
    }
    // Only a decimal token that prints back exactly: "1e3" and " 4.5" stay refused.
    let f: f64 = s.parse().ok()?;
    let n = Number::from_f64(f)?;
    (n.to_string() == s).then_some(Value::Number(n))
}

fn number_to_integer(v: &Value) -> Option<Value> {
    let f = v.as_f64()?;
    // i64 holds [-2^63, 2^63); both bounds are exact in f64.
    if !f.is_finite() || f.fract() != 0.0 || !(i64::MIN as f64..-(i64::MIN as f64)).contains(&f) {
        return None;
    }
    Some(Value::from(f as i64))
}

fn string_to_boolean(v: &Value) -> Option<Value> {
    match v.as_str()? {
        "true" => Some(Value::Bool(true)),
        "false" => Some(Value::Bool(false)),
        _ => None,
    }
}

fn string_to_null(v: &Value) -> Option<Value> {
    (v.as_str()? == "null").then_some(Value::Null)
}

/// One applied coercion, reported to the caller as `data.coerced[]`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Coerced {
    pub argument: String,
    pub instance_path: String,
    pub from: String,
    pub to: String,
}

/// A coercion that was attempted and refused (the value stayed as sent).
#[derive(Debug, Clone, PartialEq)]
pub struct Tried {
    pub instance_path: String,
    pub from: String,
    pub to: String,
}

#[derive(Debug, Clone)]
pub struct Outcome {
    pub args: Value,
    pub coerced: Vec<Coerced>,
    pub tried: Vec<Tried>,
}

/// Depth of `properties`/`items` below the top level that is still walked.
const MAX_DEPTH: usize = 1;

/// Coerces the opted-in properties of `args` per `schema`. Idempotent.
pub fn coerce_args(schema: &Value, args: &Value) -> Outcome {
    let mut out = Outcome { args: args.clone(), coerced: Vec::new(), tried: Vec::new() };
    let mut args = std::mem::take(&mut out.args);
    let mut walk = Walk { coerced: &mut out.coerced, tried: &mut out.tried };
    walk.object(schema, &mut args, "", 0);
    out.args = args;
    out
}

fn opted_in(schema: &Value) -> bool {
    schema.get("x-coerce").and_then(Value::as_bool) == Some(true)
}

struct Walk<'a> {
    coerced: &'a mut Vec<Coerced>,
    tried: &'a mut Vec<Tried>,
}

impl Walk<'_> {
    fn object(&mut self, schema: &Value, value: &mut Value, path: &str, depth: usize) {
        let (Some(props), Some(map)) = (schema.get("properties").and_then(Value::as_object), value.as_object_mut())
        else {
            return;
        };
        for (key, prop_schema) in props {
            let Some(slot) = map.get_mut(key) else { continue };
            let child = format!("{path}/{}", key.replace('~', "~0").replace('/', "~1"));
            self.visit(prop_schema, slot, &child, depth);
        }
    }

    fn visit(&mut self, schema: &Value, slot: &mut Value, path: &str, depth: usize) {
        if opted_in(schema) {
            self.one(schema, slot, path);
        }
        if depth >= MAX_DEPTH {
            return;
        }
        match slot {
            Value::Object(_) => self.object(schema, slot, path, depth + 1),
            Value::Array(items) => {
                let Some(item_schema) = schema.get("items").filter(|s| s.is_object()) else { return };
                for (i, item) in items.iter_mut().enumerate() {
                    let item_path = format!("{path}/{i}");
                    if opted_in(item_schema) {
                        self.one(item_schema, item, &item_path);
                    }
                    self.object(item_schema, item, &item_path, depth + 1);
                }
            }
            _ => {}
        }
    }

    fn one(&mut self, schema: &Value, slot: &mut Value, path: &str) {
        let declared = declared_types(schema);
        if declared.iter().any(|t| already(slot, t)) {
            return;
        }
        let from = json_type_name(slot);
        let mut first_attempt: Option<(&str, &str)> = None;
        for to in &declared {
            let Some(row) = COERCIONS.iter().find(|c| c.from == from && c.to == *to) else { continue };
            first_attempt.get_or_insert((row.from, row.to));
            if let Some(new) = (row.apply)(slot) {
                *slot = new;
                self.coerced.push(Coerced {
                    argument: path.trim_start_matches('/').split('/').next().unwrap_or_default().to_string(),
                    instance_path: path.to_string(),
                    from: row.from.to_string(),
                    to: row.to.to_string(),
                });
                return;
            }
        }
        if let Some((f, t)) = first_attempt {
            self.tried.push(Tried { instance_path: path.to_string(), from: f.to_string(), to: t.to_string() });
        }
    }
}

fn declared_types(schema: &Value) -> Vec<&str> {
    match schema.get("type") {
        Some(Value::String(t)) => vec![t.as_str()],
        Some(Value::Array(ts)) => ts.iter().filter_map(Value::as_str).collect(),
        _ => Vec::new(),
    }
}

/// True when `v` already is `declared` (JSON Schema's own reading).
fn already(v: &Value, declared: &str) -> bool {
    match (declared, v) {
        ("integer", Value::Number(n)) => n.is_i64() || n.is_u64(),
        ("number", Value::Number(_)) => true,
        (t, v) => json_type_name(v) == t,
    }
}

/// The one validation gate: coerce, then validate with jsonschema. A schema
/// jsonschema cannot compile passes through (callers' prior behaviour).
/// On success returns the args the kind must receive and what was coerced.
pub fn check_args(schema: &Value, args: &Value) -> Result<(Value, Vec<Coerced>), Box<ArgsError>> {
    let Outcome { args, coerced, tried } = coerce_args(schema, args);
    if let Ok(validator) = jsonschema::validator_for(schema)
        && let Err(e) = validator.validate(&args)
    {
        let mut err = describe_args_error(&e);
        err.coercion_tried = tried
            .iter()
            .find(|t| t.instance_path == err.instance_path)
            .map(|t| format!("{} → {}", t.from, t.to));
        return Err(Box::new(err));
    }
    Ok((args, coerced))
}

/// Adds `coerced` to an object result; an uncoerced call gets no key.
pub fn attach_coerced(value: &mut Value, coerced: &[Coerced]) {
    if coerced.is_empty() {
        return;
    }
    if let Value::Object(map) = value {
        map.insert("coerced".to_string(), serde_json::to_value(coerced).unwrap_or(Value::Null));
    }
}

/// True when `value` would be accepted for schema type `to` by a
/// [`COERCIONS`] row. Used to classify observed `args_invalid` rows.
pub fn coercible(value: &Value, to: &str) -> bool {
    let from = json_type_name(value);
    COERCIONS.iter().any(|c| c.from == from && c.to == to && (c.apply)(value).is_some())
}
