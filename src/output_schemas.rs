//! PRD-mcphost-contract-output-schemas R1: the output schema of every
//! canonical `host.*`/`billing.*` tool.
//!
//! One registry, read by every consumer: `handler::host_tools` (so the
//! `outputSchema` in `tools/list` and the contract dump come from the same
//! bytes), `api_contract::dump_contract` and `gendocs`' `Returns` block.
//! The schemas live in `contracts/host-output-schemas.v1.json`, keyed by
//! canonical tool name. They were drafted from the `structuredContent` the
//! integration suite produces (the harness records it under
//! `target/outsch/`) and are kept honest by the `outsch_*` tests, which
//! validate every captured row against them. `required` lists only keys
//! every captured row carried; a tool whose response shape depends on the
//! caller (e.g. `host.whoami` for a tenant, an admin or an implicit
//! connection) declares the union of those shapes.

use std::collections::BTreeMap;
use std::sync::{Arc, OnceLock};

use serde_json::{Map, Value};

const RAW: &str = include_str!("../contracts/host-output-schemas.v1.json");

type Registry = BTreeMap<String, Arc<Map<String, Value>>>;

fn registry() -> &'static Registry {
    static REGISTRY: OnceLock<Registry> = OnceLock::new();
    REGISTRY.get_or_init(|| {
        let parsed: BTreeMap<String, Map<String, Value>> =
            serde_json::from_str(RAW).expect("contracts/host-output-schemas.v1.json is valid JSON"); // allowlist: compile-time include_str! of a generated contract, not external input; the generated-contract test asserts it parses
        let mut parsed = parsed;
        // PRD-mcphost-task-handoff-tool R1: every `status` a host.task.*
        // output declares is the `TaskStatus` set, never a typed copy.
        for tool in crate::tasks::STATUS_TOOLS {
            if let Some(status) = parsed
                .get_mut(*tool)
                .and_then(|schema| schema.get_mut("properties"))
                .and_then(|props| props.get_mut("status"))
                .and_then(Value::as_object_mut)
            {
                status.insert("enum".to_string(), crate::tasks::TaskStatus::schema_enum());
            }
        }
        if let Err(bad) = widen_opaque(&mut parsed, OPAQUE_FIELDS) {
            panic!("OPAQUE_FIELDS: {bad}"); // allowlist: a renamed field must fail the build, never silently drop the rule; `oschv_ac01` asserts every pointer resolves
        }
        parsed
            .into_iter()
            .map(|(k, mut v)| {
                relax_object_schema(&mut v);
                (k, Arc::new(v))
            })
            .collect()
    })
}

/// PRD-mcphost-output-schema-tenant-values-opaque R1: every output path whose
/// value the TENANT defines (a chain step's result, a stored state value, a
/// tool's payload), as `(canonical tool, JSON pointer into its schema)`. The
/// corpus can only ever sample such a value, so the registry declares each
/// node free-form (`{}`) at load instead of typing it from the sample. A new
/// tenant-defined output field is one line here; `oschv_ac03` fails if one is
/// forgotten.
pub const OPAQUE_FIELDS: &[(&str, &str)] = &[
    ("host.tool.call", "/properties/steps/items/properties/result"),
    ("host.tool.call", "/properties/result"),
    ("host.runs.get", "/properties/result"),
    ("host.runs.get", "/properties/children/items/properties/result"),
    ("host.runs.list", "/properties/runs/items/properties/result"),
    ("host.runs.wait", "/properties/result"),
    ("host.runs.part", "/properties/data"),
    ("host.state.get", "/properties/value"),
    ("host.state.list", "/properties/keys/items/properties/value"),
    ("host.state.query", "/properties/rows/items"),
    ("host.tool.run", "/properties/payload"),
    ("host.tool.test", "/properties/result"),
    ("host.tool.test", "/properties/steps/items/properties/result"),
];

/// Replace the node each `(tool, pointer)` names with `{}`. `Err` names the
/// first pointer that resolves to nothing.
pub fn widen_opaque(schemas: &mut BTreeMap<String, Map<String, Value>>, fields: &[(&str, &str)]) -> Result<(), String> {
    for (tool, pointer) in fields {
        let node = schemas
            .get_mut(*tool)
            .and_then(|schema| pointer_mut(schema, pointer))
            .ok_or_else(|| format!("{tool} {pointer} does not resolve to a schema node"))?;
        *node = Value::Object(Map::new());
    }
    Ok(())
}

fn pointer_mut<'a>(schema: &'a mut Map<String, Value>, pointer: &str) -> Option<&'a mut Value> {
    let mut parts = pointer.strip_prefix('/')?.split('/');
    let mut node = schema.get_mut(parts.next()?)?;
    for part in parts {
        node = node.get_mut(part)?;
    }
    Some(node)
}

/// The corpus the schemas are drafted from cannot tell an `Option<T>` (the
/// handlers build responses with `json!`, so an absent Option is a `null`)
/// from a `T`, cannot see a type behind a field that was only ever `null`, and
/// cannot see that a pagination cursor is omitted on the last page. Prod
/// rejected its own responses for all three ("data/description must be null",
/// "data/end_user must be null", "data must have required property
/// 'next_cursor'"). So the rules are applied here, once, to every schema the
/// registry hands out -- never as a per-tool edit of the contract file -- and
/// `outsch_ac08` asserts they hold for every registered tool:
///
/// 1. a property declared `{"type": "null"}` (only ever observed null) is
///    unconstrained: a null observation carries no type information;
/// 2. every property that is not `required` also accepts `null` (the type
///    gains `"null"`; a closed `enum` stays closed);
/// 3. a `*cursor` key is never `required`.
fn relax_object_schema(schema: &mut Map<String, Value>) {
    let mut required: Vec<String> = schema
        .get("required")
        .and_then(Value::as_array)
        .map(|r| r.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
        .unwrap_or_default();
    if required.iter().any(|k| k.ends_with("cursor")) {
        required.retain(|k| !k.ends_with("cursor"));
        if required.is_empty() {
            schema.remove("required");
        } else {
            schema.insert("required".to_string(), Value::from(required.clone()));
        }
    }
    if let Some(props) = schema.get_mut("properties").and_then(Value::as_object_mut) {
        for (name, sub) in props.iter_mut() {
            let Some(sub) = sub.as_object_mut() else { continue };
            relax_object_schema(sub);
            if sub.get("type").and_then(Value::as_str) == Some("null") {
                sub.remove("type");
            } else if !required.contains(name) {
                allow_null(sub);
            }
        }
    }
    if let Some(items) = schema.get_mut("items").and_then(Value::as_object_mut) {
        relax_object_schema(items);
    }
}

/// `schema` also accepts `null`: `type: T` becomes `type: [T, "null"]`. A schema
/// with no `type` (unconstrained, or a union) is left alone, and so is an
/// `enum`: those are closed sets generated from Rust (`RUN_ROLES`,
/// `TaskStatus`), where an `Option` is not invisible.
fn allow_null(schema: &mut Map<String, Value>) {
    if schema.contains_key("enum") {
        return;
    }
    match schema.get_mut("type") {
        Some(Value::String(t)) => {
            let t = std::mem::take(t);
            schema.insert("type".to_string(), Value::from(vec![t, "null".to_string()]));
        }
        Some(Value::Array(types)) if !types.iter().any(|t| t == "null") => types.push(Value::from("null")),
        _ => {}
    }
}

/// `canonical`'s output schema, `None` when none is registered.
pub fn output_schema_for(canonical: &str) -> Option<Arc<Map<String, Value>>> {
    registry().get(canonical).cloned()
}

/// Every canonical tool name that has a registered output schema, sorted.
pub fn registered_tools() -> Vec<&'static str> {
    registry().keys().map(String::as_str).collect()
}

/// The largest `oneOf`/`anyOf` branch count anywhere in `schema` (0 when it
/// has none) -- the measure PRD AC1's "more than 3 branches" budget counts.
pub fn max_union_branches(schema: &Value) -> usize {
    match schema {
        Value::Object(map) => {
            let here = ["oneOf", "anyOf"]
                .iter()
                .filter_map(|k| map.get(*k).and_then(Value::as_array).map(Vec::len))
                .max()
                .unwrap_or(0);
            map.values().map(max_union_branches).max().unwrap_or(0).max(here)
        }
        Value::Array(items) => items.iter().map(max_union_branches).max().unwrap_or(0),
        _ => 0,
    }
}
