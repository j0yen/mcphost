//! PRD-mcphost-typescript-sdk-generated R6: the one JSON-Schema reader both
//! SDK generators (`sdk_python`, `gen::sdk_ts`) read the contract through.
//!
//! Everything a generator needs to *decide* about a schema (which types it
//! allows, whether it is nullable, which enum literals it carries, which
//! properties an object has and which are required, which numeric bounds it
//! states) is answered here, so a type cannot be optional in one SDK and
//! required in the other. The generators only differ in how they spell the
//! answer (`Optional[Literal[..]]` vs `z.enum([..]).optional()`).

use std::collections::BTreeSet;

use serde_json::Value;

/// A scalar enum literal as the contract states it.
#[derive(Clone, Debug, PartialEq)]
pub enum Lit {
    Str(String),
    Bool(bool),
    /// The number exactly as JSON spells it.
    Num(String),
}

/// One property of an object schema.
pub struct Field<'a> {
    pub wire: &'a str,
    pub required: bool,
    pub nullable: bool,
    pub schema: &'a Value,
}

/// The inclusive bounds a schema states for its single concrete type.
#[derive(Default, Debug, PartialEq)]
pub struct Bounds {
    pub min: Option<String>,
    pub max: Option<String>,
}

/// The concrete `type`s a schema allows (without `"null"`) and whether
/// `"null"` was among them.
pub fn schema_types(schema: &Value) -> (Vec<&str>, bool) {
    let mut out = Vec::new();
    match schema.get("type") {
        Some(Value::String(t)) => out.push(t.as_str()),
        Some(Value::Array(ts)) => out.extend(ts.iter().filter_map(Value::as_str)),
        _ => {}
    }
    let nullable = out.contains(&"null");
    out.retain(|t| *t != "null");
    (out, nullable)
}

/// The `enum` literals of a schema (non-scalar and `null` members dropped)
/// and whether the schema as a whole accepts `null`. `None` when there is no
/// usable enum.
pub fn enum_values(schema: &Value) -> Option<(Vec<Lit>, bool)> {
    let Some(Value::Array(vals)) = schema.get("enum") else { return None };
    let lits: Vec<Lit> = vals
        .iter()
        .filter_map(|v| match v {
            Value::String(s) => Some(Lit::Str(s.clone())),
            Value::Bool(b) => Some(Lit::Bool(*b)),
            Value::Number(n) => Some(Lit::Num(n.to_string())),
            _ => None,
        })
        .collect();
    if lits.is_empty() {
        return None;
    }
    let nullable = schema_types(schema).1 || vals.iter().any(Value::is_null);
    Some((lits, nullable))
}

/// The object's properties, sorted by wire name, with their optionality.
pub fn object_fields(schema: &Value) -> Vec<Field<'_>> {
    let required: BTreeSet<&str> = schema
        .get("required")
        .and_then(Value::as_array)
        .map(|r| r.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    let Some(props) = schema.get("properties").and_then(Value::as_object) else { return Vec::new() };
    let mut keys: Vec<&String> = props.keys().collect();
    keys.sort();
    keys.into_iter()
        .map(|wire| {
            let schema = &props[wire];
            let nullable = schema_types(schema).1 || enum_values(schema).is_some_and(|(_, n)| n);
            Field { wire, required: required.contains(wire.as_str()), nullable, schema }
        })
        .collect()
}

/// An object schema with at least one declared property.
pub fn has_properties(schema: &Value) -> bool {
    schema.get("properties").and_then(Value::as_object).is_some_and(|p| !p.is_empty())
}

/// `additionalProperties` when it is itself a schema (a string-keyed map).
pub fn map_value_schema(schema: &Value) -> Option<&Value> {
    schema.get("additionalProperties").filter(|v| v.is_object())
}

/// `additionalProperties: false`.
pub fn forbids_extra(schema: &Value) -> bool {
    schema.get("additionalProperties") == Some(&Value::Bool(false))
}

/// The `(min, max)` bounds for `ty` (`integer`/`number`: minimum/maximum;
/// `string`: minLength/maxLength; `array`: minItems/maxItems).
pub fn bounds(schema: &Value, ty: &str) -> Bounds {
    let (lo, hi) = match ty {
        "integer" | "number" => ("minimum", "maximum"),
        "string" => ("minLength", "maxLength"),
        "array" => ("minItems", "maxItems"),
        _ => return Bounds::default(),
    };
    let num = |k: &str| schema.get(k).filter(|v| v.is_number()).map(Value::to_string);
    Bounds { min: num(lo), max: num(hi) }
}

/// The bounds for a schema with exactly one concrete type, else none.
pub fn single_type_bounds(schema: &Value) -> Bounds {
    let (ts, _) = schema_types(schema);
    match ts.as_slice() {
        [t] => bounds(schema, t),
        _ => Bounds::default(),
    }
}

/// The canonical (non-alias) tools of a contract, sorted by name.
pub fn canonical(contract: &Value) -> Result<Vec<&Value>, String> {
    let tools = contract.get("tools").and_then(Value::as_array).ok_or("contract has no `tools` array")?;
    let mut v: Vec<&Value> = tools.iter().filter(|t| t.get("alias_of").is_none()).collect();
    v.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
    Ok(v)
}

/// The canonical tool names in `contract_bytes`, sorted.
pub fn canonical_names(contract_bytes: &[u8]) -> Result<Vec<String>, String> {
    let contract: Value = serde_json::from_slice(contract_bytes).map_err(|e| format!("contract is not JSON: {e}"))?;
    Ok(canonical(&contract)?.iter().filter_map(|t| t["name"].as_str().map(str::to_string)).collect())
}

/// `<major>.<sha12>`: the contract version both SDKs stamp into every file.
pub fn contract_version(contract: &Value, contract_bytes: &[u8]) -> Result<(String, u64, String), String> {
    let major = contract.get("contract_version").and_then(Value::as_u64).ok_or("contract has no contract_version")?;
    let sha = crate::api_contract::sha_hex(contract_bytes);
    let version = format!("{major}.{}", &sha[..12]);
    Ok((sha, major, version))
}
