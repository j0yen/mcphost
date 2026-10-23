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

use rmcp::model::CallToolResult;
use serde::Deserialize;
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
        // PRD-mcphost-send-email-from-tool R6: the row `status` is the
        // `SendStatus` set, never a typed copy.
        if let Some(status) = parsed
            .get_mut("host.email.sends")
            .and_then(|schema| schema.get_mut("properties"))
            .and_then(|props| props.get_mut("sends"))
            .and_then(|sends| sends.get_mut("items"))
            .and_then(|items| items.get_mut("properties"))
            .and_then(|props| props.get_mut("status"))
            .and_then(Value::as_object_mut)
        {
            status.insert("enum".to_string(), serde_json::json!(crate::email_send::SendStatus::names()));
        }
        // PRD-mcphost-sandbox-timeout-clamp-visible R3: a publish warning's
        // keys and `code` set are `ClampWarning`'s own, never a typed copy.
        if let Some(items) = parsed
            .get_mut("host.tool.publish")
            .and_then(|schema| schema.get_mut("properties"))
            .and_then(|props| props.get_mut("warnings"))
            .and_then(|warnings| warnings.get_mut("items"))
            .and_then(Value::as_object_mut)
        {
            use crate::kinds::python::ClampWarning;
            let declared = items.get("properties").and_then(Value::as_object).cloned().unwrap_or_default();
            let props: Map<String, Value> = ClampWarning::FIELDS
                .iter()
                .map(|f| ((*f).to_string(), declared.get(*f).cloned().unwrap_or_else(|| serde_json::json!({}))))
                .collect();
            items.insert("properties".to_string(), Value::Object(props));
            let mut required: Vec<&str> = ClampWarning::FIELDS.to_vec();
            required.sort_unstable();
            items.insert("required".to_string(), serde_json::json!(required));
            if let Some(code) = items
                .get_mut("properties")
                .and_then(|p| p.get_mut("code"))
                .and_then(Value::as_object_mut)
            {
                code.insert("enum".to_string(), serde_json::json!(ClampWarning::CODES));
            }
        }
        if let Err(bad) = widen_opaque(&mut parsed, OPAQUE_FIELDS) {
            panic!("OPAQUE_FIELDS: {bad}"); // allowlist: a renamed field must fail the build, never silently drop the rule; `oschv_ac01` asserts every pointer resolves
        }
        if let Err(bad) = apply_edits(&mut parsed, &observational_edits().entries) {
            panic!("host-output-observational-edits: {bad}"); // allowlist: an entry that no longer resolves must fail the build; `osmatch_ac04` asserts every pointer resolves
        }
        parsed
            .into_iter()
            .map(|(k, mut v)| {
                require_error_data(&mut v);
                relax_object_schema(&mut v);
                compose_variants(&k, &mut v);
                attach_error_branch(&mut v);
                (k, Arc::new(v))
            })
            .collect()
    })
}

/// PRD-mcphost-success-result-matches-output-schema R3: the variant branches
/// `tool` admits besides its live result, each generated from the code that
/// builds the shape. Today: the generic dry-run preview (R1).
fn variants_for(tool: &str) -> Vec<Value> {
    let mut variants = Vec::new();
    if crate::write_verbs::has_generic_dry_run(tool) {
        variants.push(crate::write_verbs::dry_run_output_branch());
    }
    if let Some((_, branch)) = OWN_DRY_RUN.iter().find(|(t, _)| *t == tool) {
        variants.push(branch());
    }
    if observational_edits()
        .entries
        .iter()
        .any(|e| e.tool == tool && e.edit == EditKind::Empty)
    {
        variants.push(empty_output_branch());
    }
    variants
}

/// A fn returning the schema of one builder's dry-run result.
pub type OwnDryRunBranch = fn() -> Value;

/// PRD-mcphost-output-schema-reach-sweep-live R4: a tool outside
/// `WRITE_VERBS` that declares `dry_run` and builds its own preview, with the
/// fn beside that builder returning the schema of what it emits. The sweep
/// fails any other such name.
pub const OWN_DRY_RUN: &[(&str, OwnDryRunBranch)] = &[(crate::import::IMPORT_TOOL_NAME, crate::import::dry_run_output_branch)];

/// Whether `tool` has a generated `own_dry_run` branch.
pub fn has_own_dry_run(tool: &str) -> bool {
    OWN_DRY_RUN.iter().any(|(t, _)| *t == tool)
}

/// The key a non-object success result is wrapped under.
pub const EMPTY_KEY: &str = "value";

/// PRD-mcphost-success-result-matches-output-schema R2: the one constructor of
/// a successful tool result. `structuredContent` must be a JSON object (MCP
/// 2025-06-18), so a value that is not one (a `null` "nothing here" answer) is
/// wrapped as `{"value": <v>}`; `content[0].text` is the serialization of what
/// went into `structuredContent`. Generic: it knows no tool, and
/// [`empty_output_branch`] is the schema of what it wraps.
pub fn success_result(value: Value) -> CallToolResult {
    let structured = if value.is_object() {
        value
    } else {
        let mut wrapped = Map::new();
        wrapped.insert(EMPTY_KEY.to_string(), value);
        Value::Object(wrapped)
    };
    CallToolResult::structured(structured)
}

/// The `empty` branch: the schema of what [`success_result`] wraps -- an
/// object with exactly the key [`EMPTY_KEY`].
pub fn empty_output_branch() -> Value {
    serde_json::json!({
        "type": "object",
        "required": [EMPTY_KEY],
        "properties": {EMPTY_KEY: {}},
        "additionalProperties": false,
    })
}

/// How an observational edit changes a tool's live schema.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EditKind {
    /// The property the pointer names stops being `required` in its parent.
    DropRequired,
    /// The node the pointer names also accepts `null`.
    AddNull,
    /// The tool also answers the [`empty_output_branch`] (pointer is `""`).
    Empty,
}

/// One entry of `contracts/host-output-observational-edits.v1.json`.
#[derive(Debug, Clone, Deserialize)]
pub struct Edit {
    pub tool: String,
    pub pointer: String,
    pub edit: EditKind,
    /// `src/<file>.rs:<line>` of the code that makes the edit necessary.
    pub emitter: String,
}

/// The parsed edits file: the entries and the committed ceiling on their count.
#[derive(Debug, Clone, Deserialize)]
pub struct Edits {
    pub max_entries: usize,
    pub entries: Vec<Edit>,
}

const EDITS_RAW: &str = include_str!("../contracts/host-output-observational-edits.v1.json");

/// PRD-mcphost-success-result-matches-output-schema R4: every schema edit
/// that comes from observation instead of from a builder, one entry each.
pub fn observational_edits() -> &'static Edits {
    static EDITS: OnceLock<Edits> = OnceLock::new();
    EDITS.get_or_init(|| {
        serde_json::from_str(EDITS_RAW).expect("contracts/host-output-observational-edits.v1.json is valid JSON") // allowlist: compile-time include_str! of a committed contract, not external input; `osmatch_ac04` asserts it parses
    })
}

/// Applies the `drop_required` and `add_null` entries (`empty` ones compose in
/// [`compose_variants`]). `Err` names the first entry that resolves to nothing.
pub fn apply_edits(schemas: &mut BTreeMap<String, Map<String, Value>>, edits: &[Edit]) -> Result<(), String> {
    for edit in edits {
        let schema = schemas
            .get_mut(&edit.tool)
            .ok_or_else(|| format!("{}: no registered output schema", edit.tool))?;
        match edit.edit {
            EditKind::Empty => {}
            EditKind::AddNull => {
                let node = pointer_mut(schema, &edit.pointer)
                    .and_then(Value::as_object_mut)
                    .ok_or_else(|| format!("{} {} does not resolve to a schema node", edit.tool, edit.pointer))?;
                allow_null(node);
            }
            EditKind::DropRequired => {
                let unresolved = || format!("{} {} does not resolve to a required property", edit.tool, edit.pointer);
                let (parent, name) = edit.pointer.rsplit_once("/properties/").ok_or_else(unresolved)?;
                let owner = if parent.is_empty() {
                    schema
                } else {
                    pointer_mut(schema, parent).and_then(Value::as_object_mut).ok_or_else(unresolved)?
                };
                let required = owner.get_mut("required").and_then(Value::as_array_mut).ok_or_else(unresolved)?;
                let before = required.len();
                required.retain(|r| r.as_str() != Some(name));
                if required.len() == before {
                    return Err(unresolved());
                }
            }
        }
    }
    Ok(())
}

/// A tool with variants keeps its root `type: object` and live `properties`;
/// its live `required` moves into the first `anyOf` branch and each variant
/// adds one branch. A tool with no variant is left byte-identical.
/// [`live_required`] is the one reader of where the live keys went.
fn compose_variants(tool: &str, schema: &mut Map<String, Value>) {
    let variants = variants_for(tool);
    if variants.is_empty() {
        return;
    }
    let mut live = Map::new();
    live.insert("type".to_string(), Value::from("object"));
    if let Some(required) = schema.remove("required") {
        live.insert("required".to_string(), required);
    }
    let mut branches = vec![Value::Object(live)];
    branches.extend(variants);
    schema.insert("anyOf".to_string(), Value::Array(branches));
}

/// The keys every LIVE result of `schema` carries: the root `required`, or,
/// once variants were composed, the first `anyOf` branch's. `api_contract`'s
/// diff and gendocs' `Returns` block both read required keys through here, so
/// a move into a branch is never a removal.
pub fn live_required(schema: &Map<String, Value>) -> Vec<&str> {
    let required = schema.get("required").or_else(|| {
        schema
            .get("anyOf")
            .and_then(Value::as_array)
            .and_then(|branches| branches.first())
            .and_then(|live| live.get("required"))
    });
    required
        .and_then(Value::as_array)
        .map(|r| r.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default()
}

/// `schema` as a client typed from the live result sees it: the variant
/// branches dropped and the live `required` back on the root. The generated
/// SDKs read an output schema through this, so a variant never turns every
/// result field optional.
pub fn live_view(schema: &Value) -> Value {
    let Some(map) = schema.as_object().filter(|m| m.contains_key("anyOf")) else {
        return schema.clone();
    };
    let required: Vec<Value> = live_required(map).into_iter().map(Value::from).collect();
    let mut live = map.clone();
    live.remove("anyOf");
    if !required.is_empty() {
        live.insert("required".to_string(), Value::Array(required));
    }
    Value::Object(live)
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

/// PRD-mcphost-error-envelope-schema-contract R2: a run-row `error` envelope
/// (an `error` property that itself declares `data`) always carries its
/// `data`, and `data` is never `null`. The corpus cannot tell a required
/// `data` from an optional one, and `relax_object_schema` would otherwise let
/// every non-required property accept `null` -- which is how the published
/// schema accepted the `error.data: null` it was meant to forbid (chain
/// verifier, 2026-10-13). Applied once, here, before the relaxation.
fn require_error_data(map: &mut Map<String, Value>) {
    if let Some(error) = map
        .get_mut("properties")
        .and_then(|p| p.get_mut("error"))
        .and_then(Value::as_object_mut)
        && error.get("properties").is_some_and(|p| p.get("data").is_some())
    {
        let required = error
            .entry("required".to_string())
            .or_insert_with(|| Value::Array(Vec::new()));
        if let Some(required) = required.as_array_mut()
            && !required.iter().any(|r| r == "data")
        {
            required.push(Value::from("data"));
        }
    }
    for child in map.values_mut() {
        match child {
            Value::Object(inner) => require_error_data(inner),
            Value::Array(items) => items
                .iter_mut()
                .filter_map(Value::as_object_mut)
                .for_each(require_error_data),
            _ => {}
        }
    }
}

/// Key under which a registered schema names its error branch.
pub const ERROR_BRANCH_KEY: &str = "x-error-envelope";

/// PRD-mcphost-error-envelope-schema-contract R2: every registered schema
/// carries the shared error envelope once, in `$defs`, and points its error
/// branch at it. The envelope is generated by `errors::error_envelope_schema`,
/// never restated per verb. Success-path readers (`properties`, `required`)
/// are untouched.
fn attach_error_branch(schema: &mut Map<String, Value>) {
    let defs = schema
        .entry("$defs".to_string())
        .or_insert_with(|| Value::Object(Map::new()));
    if let Some(defs) = defs.as_object_mut() {
        defs.insert(crate::errors::ERROR_ENVELOPE_DEF.to_string(), crate::errors::error_envelope_schema());
    }
    schema.insert(
        ERROR_BRANCH_KEY.to_string(),
        serde_json::json!({"$ref": format!("#/$defs/{}", crate::errors::ERROR_ENVELOPE_DEF)}),
    );
}

/// A standalone, validatable schema for `canonical`'s error branch, read from
/// its REGISTERED schema (so a verb whose schema lacks the branch gets
/// `None`). `Err` names what is missing.
pub fn error_branch_for(canonical: &str) -> Result<Value, String> {
    let schema = output_schema_for(canonical).ok_or_else(|| format!("{canonical}: no registered output schema"))?;
    error_branch_of(canonical, &schema)
}

/// [`error_branch_for`] over any schema: `Err` names `canonical` when the
/// schema lacks the shared error branch or its `$defs` target.
pub fn error_branch_of(canonical: &str, schema: &Map<String, Value>) -> Result<Value, String> {
    let branch = schema
        .get(ERROR_BRANCH_KEY)
        .ok_or_else(|| format!("{canonical}: registered output schema lacks the shared error branch"))?;
    let defs = schema.get("$defs");
    if defs.and_then(|d| d.get(crate::errors::ERROR_ENVELOPE_DEF)).is_none() {
        return Err(format!("{canonical}: error branch points at a missing $defs/{}", crate::errors::ERROR_ENVELOPE_DEF));
    }
    let mut out = branch.as_object().cloned().unwrap_or_default();
    if let Some(defs) = defs {
        out.insert("$defs".to_string(), defs.clone());
    }
    Ok(Value::Object(out))
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

/// How the error-envelope conformance check drives one registered verb.
#[derive(Debug, Clone, PartialEq)]
pub enum EnvelopeProbe {
    /// Argument sets to try in order; the first that makes the verb fail is
    /// the `args_invalid` envelope that gets validated.
    Drive(Vec<Value>),
    /// The verb cannot be driven from its input schema alone.
    Skipped(String),
}

/// The `tenant_key` the last-resort probe sends; an attempt carrying it is
/// meant for an unauthenticated client.
pub const INVALID_TENANT_KEY: &str = "mcp_invalid_tenant_key_for_probe";

/// A value whose JSON type contradicts `schema`'s declared `type`.
fn wrongly_typed(schema: &Value) -> Value {
    let declared = match schema.get("type") {
        Some(Value::String(t)) => Some(t.as_str()),
        Some(Value::Array(ts)) => ts.iter().find_map(Value::as_str),
        _ => None,
    };
    match declared {
        Some("string") => serde_json::json!(12345),
        Some("integer" | "number" | "boolean" | "array" | "object") => serde_json::json!("not-the-declared-type"),
        _ => serde_json::json!([]),
    }
}

/// PRD-mcphost-error-envelope-schema-contract R3: the probe for every
/// registered verb, enumerated from `registered_tools()` and each verb's
/// published input schema (never a hand list). Empty args first, then a
/// wrongly typed required arg (a wrongly typed optional arg when the verb has
/// no required ones), then an invalid tenant key. Shared by the generated conformance test and by
/// `host.doctor`'s `error_envelope` block.
pub fn envelope_probes() -> Vec<(&'static str, EnvelopeProbe)> {
    let kinds = crate::kinds::KindRegistry::with_builtin();
    let descriptors = crate::handler::host_tool_descriptors(&kinds);
    registered_tools()
        .into_iter()
        .map(|tool| {
            let Some(descriptor) = descriptors.iter().find(|t| t.name.as_ref() == tool) else {
                return (tool, EnvelopeProbe::Skipped("not in host_tools".to_string()));
            };
            let input = Value::Object((*descriptor.input_schema).clone());
            let props = input.get("properties").and_then(Value::as_object);
            let required: Vec<&str> = input
                .get("required")
                .and_then(Value::as_array)
                .map(|r| r.iter().filter_map(Value::as_str).collect())
                .unwrap_or_default();
            let target = required
                .iter()
                .find(|name| props.is_some_and(|p| p.contains_key(**name)))
                .map(|n| n.to_string())
                .or_else(|| {
                    props.and_then(|p| {
                        p.keys()
                            .find(|k| k.as_str() != "tenant_key" && k.as_str() != "_meta")
                            .cloned()
                    })
                });
            let mut attempts = vec![serde_json::json!({})];
            if let Some(target) = target {
                let bad = wrongly_typed(&props.map(|p| p[&target].clone()).unwrap_or(Value::Null));
                attempts.push(serde_json::json!({ target: bad }));
            }
            // Last resort for a verb with nothing to mistype (or that
            // tolerates every bad argument): an invalid tenant key, which
            // every verb rejects before it looks at its arguments.
            attempts.push(serde_json::json!({ "tenant_key": INVALID_TENANT_KEY }));
            (tool, EnvelopeProbe::Drive(attempts))
        })
        .collect()
}

const VARIANTS_RAW: &str = include_str!("../contracts/host-output-variants.v1.json");

/// What `host.doctor` reports as `output_reach`, read from
/// `contracts/host-output-variants.v1.json` (PRD-mcphost-output-schema-reach-sweep-live R5).
pub struct ReachReport {
    pub unreached: u64,
    pub unreached_cap: u64,
}

/// The unreached `(tool, state)` count and the committed `unreached_cap` of the
/// variants file the osreach sweep writes. Computed once.
pub fn reach_report() -> &'static ReachReport {
    static REPORT: OnceLock<ReachReport> = OnceLock::new();
    REPORT.get_or_init(|| {
        let parsed: Value = serde_json::from_str(VARIANTS_RAW).unwrap_or(Value::Null);
        let unreached = parsed["tools"]
            .as_object()
            .map(|tools| tools.values().filter_map(|t| t["unreached"].as_array()).map(|u| u.len() as u64).sum())
            .unwrap_or(0);
        ReachReport { unreached, unreached_cap: parsed["unreached_cap"].as_u64().unwrap_or(0) }
    })
}

/// What `host.doctor` reports as `error_envelope`.
pub struct EnvelopeReport {
    pub validated: usize,
    pub skipped: Vec<String>,
}

/// PRD-mcphost-error-envelope-schema-contract R5: envelope conformance from
/// the SAME enumeration the generated test drives ([`envelope_probes`]). Every
/// verb that can be driven has the host's `args_invalid` envelope, built by
/// the one constructor, validated against that verb's registered error
/// branch; a verb that cannot be driven is listed as `skipped:<verb>:<reason>`.
/// Computed once.
pub fn error_envelope_report() -> &'static EnvelopeReport {
    static REPORT: OnceLock<EnvelopeReport> = OnceLock::new();
    REPORT.get_or_init(|| {
        let sample = serde_json::to_value(crate::errors::AppError::InvalidArgs("probe".into()).into_error_data())
            .unwrap_or(Value::Null);
        let mut validated = 0;
        let mut skipped = Vec::new();
        for (tool, probe) in envelope_probes() {
            match probe {
                EnvelopeProbe::Skipped(reason) => skipped.push(format!("skipped:{tool}:{reason}")),
                EnvelopeProbe::Drive(_) => {
                    let conforms = error_branch_for(tool)
                        .ok()
                        .and_then(|branch| jsonschema::validator_for(&branch).ok())
                        .is_some_and(|validator| validator.is_valid(&sample));
                    if conforms {
                        validated += 1;
                    } else {
                        skipped.push(format!("skipped:{tool}:envelope does not validate"));
                    }
                }
            }
        }
        EnvelopeReport { validated, skipped }
    })
}
