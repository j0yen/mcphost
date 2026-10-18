//! PRD-mcphost-input-schema-is-the-validator R3: a `host.*` call's args are
//! validated against the tool's own `inputSchema` before its handler runs,
//! the way a tenant tool's args already are (`describe_args_error`). The
//! descriptor is the one place a structural constraint lives; the handler's
//! hand checks stay behind it for one release (R4 proves them unreachable for
//! schema-valid input). Only descriptors marked [`VALIDATED_KEY`] validate: a
//! descriptor whose looser or stricter wording than its handler has not been
//! reconciled (see `fldp_*`, `publishfirsttry_*`, `pgtr_ac07_*`) keeps its
//! handler's own refusal.

use crate::errors::{ARGS_INVALID, AppError};
use crate::handler::{admin_tool_descriptors, host_tool_descriptors};
use crate::kinds::{KindRegistry, describe_args_error};
use jsonschema::{Validator, error::ValidationErrorKind};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

/// The top-level schema key marking a descriptor as the validator for its
/// tool. Generated into the descriptor by `host_schema_checked`/
/// `host_schema_extra`, so the set of validated tools cannot drift from the
/// descriptors; a tool without it keeps its handler's own checks and error
/// taxonomy (`invalid_tool_name`, `invalid_params`, clamped limits, ...).
pub const VALIDATED_KEY: &str = "x-mcphost-validated";

/// Handler bodies entered (after validation passed), per (tenant namespace,
/// tool). A test reads its own tenant's count before and after a violating
/// call to prove the handler never ran; keyed by tenant so concurrent tests in
/// one binary cannot disturb each other.
type RunCounts = Mutex<HashMap<(String, String), u64>>;
static HANDLER_RUNS: OnceLock<RunCounts> = OnceLock::new();

/// Records that `tool`'s handler body is about to run for `namespace`.
pub fn note_handler_run(namespace: &str, tool: &str) {
    let mut runs = HANDLER_RUNS.get_or_init(Default::default).lock().unwrap_or_else(|e| e.into_inner());
    *runs.entry((namespace.to_string(), tool.to_string())).or_insert(0) += 1;
}

/// How many times `tool`'s handler body has run for `namespace`.
pub fn handler_runs(namespace: &str, tool: &str) -> u64 {
    let runs = HANDLER_RUNS.get_or_init(Default::default).lock().unwrap_or_else(|e| e.into_inner());
    runs.get(&(namespace.to_string(), tool.to_string())).copied().unwrap_or(0)
}

struct Compiled {
    schema: Value,
    validator: Validator,
}

type Table = Arc<HashMap<String, Compiled>>;

/// One compiled table per distinct registry: `host.tool.publish`'s `kind`
/// enum is the registry's own set, so two registries cannot share validators.
static TABLES: OnceLock<Mutex<HashMap<Vec<&'static str>, Table>>> = OnceLock::new();

fn table_for(kinds: &KindRegistry) -> Table {
    let key = kinds.publish_kind_enum();
    let cache = TABLES.get_or_init(Default::default);
    if let Some(t) = cache.lock().unwrap_or_else(|e| e.into_inner()).get(&key) {
        return t.clone();
    }
    let compiled: HashMap<String, Compiled> = host_tool_descriptors(kinds)
        .into_iter()
        .chain(admin_tool_descriptors())
        .filter(|tool| tool.input_schema.get(VALIDATED_KEY) == Some(&Value::Bool(true)))
        .filter_map(|tool| {
            let mut schema = Value::Object((*tool.input_schema).clone());
            // A paged verb's `limit` above its max is CLAMPED and reported
            // (`_meta.mcphost.limit_clamped`, PRD-mcphost-paged-trait-on-every-
            // list-verb requirement 7), never refused: the published schema
            // keeps `maximum` as documentation, the validator must not enforce it.
            if crate::paged::paged_spec(tool.name.as_ref()).is_some()
                && let Some(limit) = schema.pointer_mut("/properties/limit").and_then(Value::as_object_mut)
            {
                limit.remove("maximum");
            }
            let validator = jsonschema::validator_for(&schema).ok()?;
            Some((tool.name.to_string(), Compiled { schema, validator }))
        })
        .collect();
    let table = Arc::new(compiled);
    cache.lock().unwrap_or_else(|e| e.into_inner()).insert(key, table.clone());
    table
}

/// The keyword a schema violation came from: the last segment of its schema
/// path (`/properties/level/enum` -> `enum`).
fn constraint_of(schema_path: &str) -> String {
    schema_path.rsplit('/').find(|s| !s.is_empty()).unwrap_or("schema").to_string()
}

/// Whether `err` is a violation of the generated `fields` property of a
/// projecting verb. `paged::parse_fields` is the one owner of `fields`
/// semantics -- an unknown name is `field_unknown {field, nearest, valid}`
/// (the one set the fldp contract pins), duplicates collapse, an empty or
/// malformed list is `args_invalid` -- and it runs first in the handler, so the
/// validator leaves these to it exactly as it leaves `additionalProperties` to
/// `check_unknown_argument`. The set of such verbs is `paged::fields_for`, the
/// same source that generates the property, so a verb moved onto the checked
/// path later keeps its `field_unknown` without a special case.
fn left_to_projection(tool: &str, err: &jsonschema::ValidationError<'_>) -> bool {
    let at = err.instance_path.as_str();
    crate::paged::fields_for(tool).is_some() && (at == "/fields" || at.starts_with("/fields/"))
}

/// Validates `args` against `tool`'s descriptor. `Ok` when the tool has no
/// descriptor here (not a `host.*` tool) or the args satisfy it. An unlisted
/// argument (`additionalProperties`) is left to `check_unknown_argument`'s own
/// `unknown_argument` refusal, which names the known set.
pub fn validate_host_args(kinds: &KindRegistry, tool: &str, args: &Value) -> Result<(), AppError> {
    let table = table_for(kinds);
    let Some(compiled) = table.get(tool) else { return Ok(()) };
    let Some(err) = compiled
        .validator
        .iter_errors(args)
        .find(|e| constraint_of(e.schema_path.as_str()) != "additionalProperties" && !left_to_projection(tool, e))
    else {
        return Ok(());
    };
    let args_err = describe_args_error(&err);
    let constraint = constraint_of(err.schema_path.as_str());
    let expected = compiled.schema.pointer(err.schema_path.as_str()).cloned().unwrap_or(Value::Null);
    // A missing property is named by the path, not the object it is missing from.
    let path = match &err.kind {
        ValidationErrorKind::Required { property } => {
            format!("{}/{}", err.instance_path.as_str(), property.as_str().unwrap_or_default())
        }
        _ if err.instance_path.as_str().is_empty() => "/".to_string(),
        _ => err.instance_path.as_str().to_string(),
    };
    let docs = format!("docs/tools.md#{}", tool.replace('.', ""));
    let mut data = args_err.data();
    if let Some(obj) = data.as_object_mut() {
        obj.insert("tool".into(), json!(tool));
        obj.insert("path".into(), json!(path));
        obj.insert("constraint".into(), json!(constraint));
        obj.insert("expected".into(), expected);
        obj.insert("docs".into(), json!(docs));
    }
    Err(AppError::Structured {
        code: ARGS_INVALID,
        message: format!("{tool}: {} ({constraint} at {path}); see {docs}", err),
        data,
    })
}
