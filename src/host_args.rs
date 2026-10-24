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
/// tool. Generated into the descriptor by [`mark_validated`], so the set of validated tools cannot drift from the
/// descriptors; a tool without it keeps its handler's own checks and error
/// taxonomy (`invalid_tool_name`, `invalid_params`, clamped limits, ...).
pub const VALIDATED_KEY: &str = "x-mcphost-validated";

/// PRD-mcphost-host-args-validated-by-default R2: the tools that keep their
/// handler's own refusal instead of validating by their descriptor, as
/// `(tool, reason)` where the reason names the `tests/` file pinning that
/// refusal. Shrink-only: `vbd_ac01` fails when an entry names an absent tool or
/// file, or when the list outgrows [`UNVALIDATED_SNAPSHOT`].
pub const UNVALIDATED: &[(&str, &str)] = &[
    // The `allOf` spec branches disagree with `Kind::describe`'s own refusal
    // (`invalid_spec`, `invalid_tool_name`, `unknown_kind` with their `field`/
    // `expected`/`example`), which this test pins; the two alias names are
    // clones of the canonical descriptor and so opt out with it.
    ("host.tool.publish", "publishfirsttry_ac02_structured_error_fields.rs"),
    ("host.tool_publish", "publishfirsttry_ac02_structured_error_fields.rs"),
    ("host_tool_publish", "publishfirsttry_ac02_structured_error_fields.rs"),
];

/// The `UNVALIDATED.len()` at merge; lowering it is the only edit that lets the
/// list shrink, raising it is refused by review (3 is the ceiling).
pub const UNVALIDATED_SNAPSHOT: usize = 3;

/// PRD-mcphost-schema-bound-agreement R3/R5: one argument bound a handler
/// enforces with `args_invalid` that the advertised schema cannot (or does not)
/// state. `pointer` is the argument's JSON pointer, `konst` the handler's bound
/// const ("" when the refusal has none), `reason` is `byte-bound` (JSON Schema's
/// `maxLength` counts code points, the handler counts bytes) or prose, and
/// `test` names the `tests/` file pinning that the handler refusal still fires.
#[derive(Clone, Copy, Debug)]
pub struct SchemaValidRefusal {
    pub tool: &'static str,
    pub pointer: &'static str,
    pub konst: &'static str,
    pub reason: &'static str,
    pub test: &'static str,
}

/// Shrink-only residue of schema-valid calls a handler still refuses. `sba_ac01`
/// and `sba_ac02` accept a handler `args`/`spec` refusal or an unmapped bound
/// const only through an entry here; `sba_ac03` pins each entry.
pub const SCHEMA_VALID_REFUSALS: &[SchemaValidRefusal] = &[
    SchemaValidRefusal { tool: "admin.usage.top", pointer: "/by", konst: "", reason: "enum: `by` is one of tool|tenant; the handler names the set", test: "sba_ac03_schema_valid_refusals_still_fire.rs" },
    SchemaValidRefusal { tool: "admin.plan_set", pointer: "/plan", konst: "", reason: "enum: plan names come from the plan table at call time", test: "sba_ac03_schema_valid_refusals_still_fire.rs" },
    SchemaValidRefusal { tool: "admin.tenants_set_synthetic", pointer: "/label", konst: "", reason: "format: the synthetic label is validated by the handler's own charset rule", test: "sba_ac03_schema_valid_refusals_still_fire.rs" },
    SchemaValidRefusal { tool: "admin.mesh.thread", pointer: "/cursor", konst: "", reason: "format: an opaque cursor the pager decodes", test: "sba_ac03_schema_valid_refusals_still_fire.rs" },
    SchemaValidRefusal { tool: "admin.ban.add", pointer: "/ttl", konst: "", reason: "format: a duration string (30m, 24h, 7d) parsed by the handler", test: "sba_ac03_schema_valid_refusals_still_fire.rs" },
    SchemaValidRefusal { tool: "admin.oauth.client_block", pointer: "/cimd_host", konst: "", reason: "relational: exactly one of client_id or cimd_host is required", test: "sba_ac03_schema_valid_refusals_still_fire.rs" },
    SchemaValidRefusal { tool: "admin.oauth.client_unblock", pointer: "/cimd_host", konst: "", reason: "relational: exactly one of client_id or cimd_host is required", test: "sba_ac03_schema_valid_refusals_still_fire.rs" },
    SchemaValidRefusal { tool: "admin.feedback.themes", pointer: "/since", konst: "", reason: "format: a duration string (24h, 7d) parsed by the handler", test: "sba_ac03_schema_valid_refusals_still_fire.rs" },
    SchemaValidRefusal { tool: "host.calls.list", pointer: "/credential", konst: "", reason: "enum: the credential set is the handler's own list", test: "sba_ac03_schema_valid_refusals_still_fire.rs" },
    SchemaValidRefusal { tool: "host.calls.list", pointer: "/cursor", konst: "", reason: "format: an opaque cursor the pager decodes", test: "sba_ac03_schema_valid_refusals_still_fire.rs" },
    SchemaValidRefusal { tool: "host.help.get", pointer: "/anchor", konst: "", reason: "format: a `<path>#<slug>` anchor resolved against the docs index", test: "sba_ac03_schema_valid_refusals_still_fire.rs" },
    SchemaValidRefusal { tool: "host.usage", pointer: "/by", konst: "", reason: "enum: `by` is one of tool|caller|end_user|key; the handler names the set", test: "sba_ac03_schema_valid_refusals_still_fire.rs" },
    SchemaValidRefusal { tool: "host.changelog", pointer: "/since", konst: "", reason: "format: a semver or `last_seen` parsed by the handler", test: "sba_ac03_schema_valid_refusals_still_fire.rs" },
    SchemaValidRefusal { tool: "host.secret.leak_test", pointer: "/name", konst: "", reason: "format: a secret name matching ^[a-zA-Z0-9_]+$", test: "sba_ac03_schema_valid_refusals_still_fire.rs" },
    SchemaValidRefusal { tool: "host.agent.profile_set", pointer: "/description", konst: "MAX_DESCRIPTION_BYTES", reason: "byte-bound", test: "sba_ac03_schema_valid_refusals_still_fire.rs" },
    SchemaValidRefusal { tool: "host.agent.profile_set", pointer: "/tags", konst: "MAX_TAG_BYTES", reason: "byte-bound", test: "sba_ac03_schema_valid_refusals_still_fire.rs" },
    SchemaValidRefusal { tool: "host.agent.contact_request", pointer: "/note", konst: "MAX_NOTE_BYTES", reason: "byte-bound", test: "sba_ac03_schema_valid_refusals_still_fire.rs" },
    SchemaValidRefusal { tool: "host.email.send", pointer: "/subject", konst: "SUBJECT_MAX_BYTES", reason: "byte-bound", test: "sba_ac03_schema_valid_refusals_still_fire.rs" },
    SchemaValidRefusal { tool: "host.email.send", pointer: "/text", konst: "TEXT_MAX_BYTES", reason: "byte-bound", test: "sba_ac03_schema_valid_refusals_still_fire.rs" },
    SchemaValidRefusal { tool: "host.notify.send", pointer: "/text", konst: "TEXT_MAX_BYTES", reason: "byte-bound", test: "sba_ac03_schema_valid_refusals_still_fire.rs" },
    SchemaValidRefusal { tool: "host.note.put", pointer: "/text", konst: "TEXT_MAX_BYTES", reason: "byte-bound", test: "sba_ac03_schema_valid_refusals_still_fire.rs" },
    SchemaValidRefusal { tool: "host.table.count", pointer: "/where", konst: "", reason: "format: a boolean SQL expression parsed by the handler", test: "sba_ac03_schema_valid_refusals_still_fire.rs" },
    SchemaValidRefusal { tool: "host.lineage.blast_radius", pointer: "/change_kind", konst: "", reason: "enum: the change kinds are the handler's own list", test: "sba_ac03_schema_valid_refusals_still_fire.rs" },
    SchemaValidRefusal { tool: "host.docs.put", pointer: "/mime", konst: "", reason: "enum: the allowed mimes are the handler's own list", test: "sba_ac03_schema_valid_refusals_still_fire.rs" },
    SchemaValidRefusal { tool: "host.blob.put", pointer: "/url", konst: "", reason: "relational: exactly one of content_base64 or url is required", test: "sba_ac03_schema_valid_refusals_still_fire.rs" },
    SchemaValidRefusal { tool: "host.docs.search", pointer: "/mode", konst: "", reason: "enum: the search modes are the handler's own list", test: "sba_ac03_schema_valid_refusals_still_fire.rs" },
    SchemaValidRefusal { tool: "host.docs.index_config", pointer: "/provider", konst: "", reason: "enum: the provider set is the handler's own list", test: "sba_ac03_schema_valid_refusals_still_fire.rs" },
    SchemaValidRefusal { tool: "host.uptime.create", pointer: "/name", konst: "", reason: "format: an identifier matching ^[A-Za-z_][A-Za-z0-9_]{0,39}$", test: "sba_ac03_schema_valid_refusals_still_fire.rs" },
    SchemaValidRefusal { tool: "host.webhook_inbox.create", pointer: "/name", konst: "", reason: "format: a tool name matching ^[a-z][a-z0-9_]{1,40}$", test: "sba_ac03_schema_valid_refusals_still_fire.rs" },
    SchemaValidRefusal { tool: "host.runs.list", pointer: "/cursor", konst: "", reason: "format: an opaque cursor the pager decodes", test: "sba_ac03_schema_valid_refusals_still_fire.rs" },
    SchemaValidRefusal { tool: "billing.checkout", pointer: "/source", konst: "", reason: "enum: the checkout sources are the handler's own list", test: "sba_ac03_schema_valid_refusals_still_fire.rs" },
    SchemaValidRefusal { tool: "host.msg.inbox", pointer: "/cursor", konst: "", reason: "format: an opaque cursor the pager decodes", test: "sba_ac03_schema_valid_refusals_still_fire.rs" },
    SchemaValidRefusal { tool: "host.msg.thread", pointer: "/cursor", konst: "", reason: "format: an opaque cursor the pager decodes", test: "sba_ac03_schema_valid_refusals_still_fire.rs" },
    SchemaValidRefusal { tool: "host.msg.wait", pointer: "/cursor", konst: "", reason: "format: an opaque cursor the pager decodes", test: "sba_ac03_schema_valid_refusals_still_fire.rs" },
    SchemaValidRefusal { tool: "host.ask.create", pointer: "/options", konst: "", reason: "uniqueness: options must be distinct; the placeholder repeats one entry", test: "sba_ac03_schema_valid_refusals_still_fire.rs" },
    SchemaValidRefusal { tool: "host.notify.send", pointer: "/link", konst: "", reason: "format: an absolute http(s) URL parsed by the handler", test: "sba_ac03_schema_valid_refusals_still_fire.rs" },
    SchemaValidRefusal { tool: "host.channel.read", pointer: "/cursor", konst: "", reason: "format: an opaque cursor the pager decodes", test: "sba_ac03_schema_valid_refusals_still_fire.rs" },
    SchemaValidRefusal { tool: "host.oauth.scope_set", pointer: "/name", konst: "", reason: "format: a scope name matching ^[a-z][a-z0-9_:.-]{0,40}$", test: "sba_ac03_schema_valid_refusals_still_fire.rs" },
    SchemaValidRefusal { tool: "host.oauth.audit", pointer: "/cursor", konst: "", reason: "format: an opaque cursor the pager decodes", test: "sba_ac03_schema_valid_refusals_still_fire.rs" },
    SchemaValidRefusal { tool: "host.oauth.audit_export", pointer: "/until", konst: "", reason: "relational: `until` must not be before `since`", test: "sba_ac03_schema_valid_refusals_still_fire.rs" },
    SchemaValidRefusal { tool: "host.enduser.list", pointer: "/cursor", konst: "", reason: "format: an opaque cursor the pager decodes", test: "sba_ac03_schema_valid_refusals_still_fire.rs" },
    SchemaValidRefusal { tool: "host.enduser.audit", pointer: "/cursor", konst: "", reason: "format: an opaque cursor the pager decodes", test: "sba_ac03_schema_valid_refusals_still_fire.rs" },
    SchemaValidRefusal { tool: "host.connect.start", pointer: "/provider", konst: "", reason: "enum: the providers are the catalog loaded at start", test: "sba_ac03_schema_valid_refusals_still_fire.rs" },
    SchemaValidRefusal { tool: "host.connect.revoke", pointer: "/provider", konst: "", reason: "enum: the providers are the catalog loaded at start", test: "sba_ac03_schema_valid_refusals_still_fire.rs" },
    SchemaValidRefusal { tool: "host.vault.provider_set", pointer: "/preset", konst: "", reason: "enum: the presets are the catalog loaded at start", test: "sba_ac03_schema_valid_refusals_still_fire.rs" },
    SchemaValidRefusal { tool: "host.vault.connect_link", pointer: "/end_user", konst: "", reason: "enum: the only accepted value is the literal self", test: "sba_ac03_schema_valid_refusals_still_fire.rs" },
    SchemaValidRefusal { tool: "host.vault.disconnect", pointer: "/end_user", konst: "", reason: "enum: the only accepted value is the literal self", test: "sba_ac03_schema_valid_refusals_still_fire.rs" },
    SchemaValidRefusal { tool: "host.sync.create", pointer: "/primary_key", konst: "", reason: "format: an identifier matching ^[A-Za-z_][A-Za-z0-9_]{0,63}$", test: "sba_ac03_schema_valid_refusals_still_fire.rs" },
    SchemaValidRefusal { tool: "host.sync.create", pointer: "/table", konst: "", reason: "format: an identifier matching ^[A-Za-z_][A-Za-z0-9_]{0,63}$", test: "sba_ac03_schema_valid_refusals_still_fire.rs" },
    SchemaValidRefusal { tool: "host.claim.dns.start", pointer: "/domain", konst: "", reason: "format: a DNS name checked by the handler", test: "sba_ac03_schema_valid_refusals_still_fire.rs" },
    SchemaValidRefusal { tool: "host.claim.dns.check", pointer: "/domain", konst: "", reason: "format: a DNS name checked by the handler", test: "sba_ac03_schema_valid_refusals_still_fire.rs" },
];

/// The `SCHEMA_VALID_REFUSALS.len()` at merge; adding an entry needs this bump,
/// which review refuses (the residue only shrinks).
pub const SCHEMA_VALID_REFUSALS_SNAPSHOT: usize = 51;

/// `sba_ac02`: a guard counts when one of these constructors sits within
/// [`SCAN_WINDOW_LINES`] lines of a `> CONST` / `>= CONST` comparison.
pub const INVALID_ARGS_CTORS: &[&str] = &["AppError::InvalidArgs(", "InvalidArgs("];

/// Lines either side of a comparison in which an [`INVALID_ARGS_CTORS`] entry
/// makes it a handler guard (tuned against the cited tree).
pub const SCAN_WINDOW_LINES: usize = 3;

/// R1: marks every descriptor not named in [`UNVALIDATED`] with
/// [`VALIDATED_KEY`]; the one writer of the key.
pub fn mark_validated(tools: &mut [rmcp::model::Tool]) {
    for tool in tools.iter_mut() {
        if UNVALIDATED.iter().any(|(name, _)| *name == tool.name.as_ref()) {
            continue;
        }
        std::sync::Arc::make_mut(&mut tool.input_schema).insert(VALIDATED_KEY.to_string(), Value::Bool(true));
    }
}

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
    /// `None` for a tool named in [`UNVALIDATED`]: it keeps its handler's own
    /// refusals, but still answers an unlisted argument as `unknown_argument`.
    validator: Option<Validator>,
    /// The descriptor's own `properties`, the `data.known` of `unknown_argument`.
    known: Vec<String>,
    /// A clone published under another name of the same verb (an alias, the
    /// flattened `a_b_c` form): not a verb of its own for `accepted_by`.
    alias: bool,
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
        .filter_map(|tool| {
            let schema = Value::Object((*tool.input_schema).clone());
            let validator = if tool.input_schema.get(VALIDATED_KEY) == Some(&Value::Bool(true)) {
                Some(jsonschema::validator_for(&schema).ok()?)
            } else {
                None
            };
            let known = tool
                .input_schema
                .get("properties")
                .and_then(Value::as_object)
                .map(|p| p.keys().cloned().collect())
                .unwrap_or_default();
            let alias = tool.meta.as_ref().is_some_and(|m| m.0.contains_key("alias_of"));
            Some((tool.name.to_string(), Compiled { schema, validator, known, alias }))
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

/// PRD-mcphost-host-args-validated-by-default R3: a sub-contract a module owns
/// and refuses (or clamps) itself, registered by the OWNER so the validator
/// does not restate it. `validate_host_args` skips a violation only when some
/// owner `applies` to the tool and the violation is at `path` (or below it)
/// under one of `keywords` (`"*"` = any); `codes` are the error codes the
/// owner answers with (empty: the owner succeeds, e.g. a clamped `limit`).
/// `vbd_ac04` sends each owner a violating call on every tool it applies to
/// and asserts the reply is the owner's own outcome, so a registration the
/// owner does not honour cannot stand.
#[derive(Clone, Copy)]
pub struct ArgOwner {
    pub applies: fn(&str) -> bool,
    pub path: &'static str,
    pub keywords: &'static [&'static str],
    pub codes: &'static [&'static str],
}

/// Every registration, collected from the owning modules.
pub fn arg_owners() -> &'static [ArgOwner] {
    static OWNERS: OnceLock<Vec<ArgOwner>> = OnceLock::new();
    OWNERS.get_or_init(|| {
        [crate::paged::ARG_OWNERS, crate::errors::ARG_OWNERS, crate::plans::ARG_OWNERS, crate::handler::ARG_OWNERS]
            .into_iter()
            .flatten()
            .copied()
            .collect()
    })
}

/// Whether `owner` owns a violation of `keyword` at the instance path `at` of `tool`.
pub fn owner_covers(owner: &ArgOwner, tool: &str, at: &str, keyword: &str) -> bool {
    (owner.applies)(tool)
        && (owner.path.is_empty() || at == owner.path || at.strip_prefix(owner.path).is_some_and(|r| r.starts_with('/')))
        && owner.keywords.iter().any(|k| *k == "*" || *k == keyword)
}

/// Whether any of `owners` owns the violation (the registry reader).
pub fn owned_by(owners: &[ArgOwner], tool: &str, at: &str, keyword: &str) -> bool {
    owners.iter().any(|o| owner_covers(o, tool, at, keyword))
}

/// Validates `args` against `tool`'s descriptor. `Ok` when the tool has no
/// descriptor here (not a `host.*` tool) or the args satisfy it. A violation an
/// [`ArgOwner`] owns is left to that owner.
pub(crate) fn validate_host_args(kinds: &KindRegistry, tool: &str, args: &Value) -> Result<(), AppError> {
    let table = table_for(kinds);
    let Some(compiled) = table.get(tool) else { return Ok(()) };
    if let Some(refusal) = schema_refusal(compiled, tool, args) {
        return Err(refusal);
    }
    // The one dispatch-level `unknown_argument` call site (R5): the
    // `additionalProperties` owner registered in `errors::ARG_OWNERS`, for every
    // control-plane tool, validated or not.
    crate::errors::check_unknown_argument(args, tool, &compiled.known).map_err(|e| with_remedy(e, &table, tool, compiled))
}

/// The `unknown_argument` refusal plus its GENERATED remedy: `accepted_by`, the
/// verbs of the same family whose published `properties` include the key
/// (from the same descriptors `known` comes from), and `did_you_mean`, the
/// nearest declared property of the called verb (<= 3 edits, else null).
fn with_remedy(err: AppError, table: &Table, tool: &str, compiled: &Compiled) -> AppError {
    let AppError::Structured { code, message, mut data } = err else { return err };
    let key = data.get("field").and_then(Value::as_str).unwrap_or_default().to_string();
    let admin = tool.starts_with("admin.");
    let mut accepted_by: Vec<&str> = table
        .iter()
        .filter(|(name, c)| {
            !c.alias && name.as_str() != tool && name.starts_with("admin.") == admin && c.known.contains(&key)
        })
        .map(|(name, _)| name.as_str())
        .collect();
    accepted_by.sort_unstable();
    let declared: Vec<&str> = compiled.known.iter().map(String::as_str).filter(|k| *k != "tenant_key").collect();
    if let Some(obj) = data.as_object_mut() {
        obj.insert("accepted_by".into(), json!(accepted_by));
        obj.insert("did_you_mean".into(), json!(crate::paged::nearest_field(&key, &declared)));
    }
    AppError::Structured { code, message, data }
}

/// The `args_invalid` refusal for the first violation of `tool`'s descriptor
/// that no [`ArgOwner`] owns, `None` when there is none (or the tool is not
/// validated).
fn schema_refusal(compiled: &Compiled, tool: &str, args: &Value) -> Option<AppError> {
    let validator = compiled.validator.as_ref()?;
    let mut errors = validator
        .iter_errors(args)
        .filter(|e| !owned_by(arg_owners(), tool, e.instance_path.as_str(), &constraint_of(e.schema_path.as_str())));
    let first = errors.next()?;
    // A wrong-typed value also fails its `enum`; the type is the root cause, so
    // it is the violation reported (the same instance path, `type` first).
    let err = if constraint_of(first.schema_path.as_str()) == "type" {
        first
    } else {
        let at = first.instance_path.as_str().to_string();
        let typed = errors.find(|e| e.instance_path.as_str() == at && constraint_of(e.schema_path.as_str()) == "type");
        typed.unwrap_or(first)
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
    Some(AppError::Structured {
        code: ARGS_INVALID,
        message: format!("{tool}: {} ({constraint} at {path}); see {docs}", err),
        data,
    })
}
