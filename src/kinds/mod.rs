//! Tool execution kinds: the `Kind` trait and its registry.
//!
//! `mcphost` publishes tools of a *kind* (`echo` is the reference
//! implementation shipped here; REST-wrapper and code kinds are separate
//! PRDs that extend this crate's registry via `build_into`). A `Kind` knows
//! how to validate a tenant-supplied `spec`, describe the tool it produces
//! for `tools/list`, and execute a call against it.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use jsonschema::error::{TypeKind, ValidationErrorKind};
use serde_json::{Map, Value, json};

pub mod conformance;
pub mod docs;
pub mod echo;
pub mod http;
pub mod infer;
pub mod python;

/// Error returned by a [`Kind`]'s methods. Distinct from the JSON-RPC error
/// the host ultimately answers with; `mcphost::errors` maps this onto that.
#[derive(Debug, thiserror::Error, Clone)]
pub enum KindError {
    #[error("invalid spec: {0}")]
    InvalidSpec(String),
    #[error("invalid arguments: {0}")]
    InvalidArgs(String),
    #[error("execution failed: {0}")]
    Exec(String),
    /// A kind-specific structured error carrying a stable, machine-readable
    /// `code` its own PRD/acceptance criteria name directly (e.g.
    /// `host_not_allowed`, `upstream_status`, `template_error`) rather than
    /// this crate's generic `invalid_spec`/`invalid_args`/`internal`
    /// taxonomy. `data` is merged into the JSON-RPC error's `data` field
    /// (alongside `error_code`) so a caller can match extra fields like
    /// `retry_after_s` or `status` without parsing prose. The `http` kind
    /// (`mcphost::kinds::http`) is this variant's first user.
    #[error("{message}")]
    Structured {
        code: &'static str,
        message: String,
        data: Value,
    },
}

impl KindError {
    /// A [`KindError::Structured`] with no extra `data` fields beyond
    /// `error_code`.
    pub fn structured(code: &'static str, message: impl Into<String>) -> Self {
        Self::Structured {
            code,
            message: message.into(),
            data: Value::Null,
        }
    }

    /// A [`KindError::Structured`] carrying extra `data` fields (must be a
    /// JSON object; merged into the error's `data` alongside `error_code`).
    pub fn structured_with(code: &'static str, message: impl Into<String>, data: Value) -> Self {
        Self::Structured {
            code,
            message: message.into(),
            data,
        }
    }
}

/// JSON type name for a [`Value`], in the same vocabulary JSON Schema's
/// `type` keyword uses (`"integer"` distinct from `"number"`, matching
/// [`jsonschema`]'s own [`jsonschema::primitive_type::PrimitiveType`]
/// naming) -- so an `args_coercion` error's `actual_type` and
/// `expected_type` are directly comparable strings.
fn json_type_name(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(n) if n.is_i64() || n.is_u64() => "integer",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

/// PRD-mcphost-python-kind-runtime requirement 1/AC3: the host's only
/// argument-type check across every kind is [`jsonschema`] validation at
/// call time -- there is no separate coercion step anywhere in this crate
/// (a kind's own module doc may say "schema-driven argument coercion", but
/// that always turns out to mean *schema inference*, never runtime type
/// coercion; args reach a kind's `call` exactly as the caller sent them).
/// This validation gate is therefore the `args_coercion` phase the AC
/// names: a call whose arguments don't match the published `args_schema`
/// fails here, before any kind-specific work (a sandbox spawn, an upstream
/// HTTP request, ...) even starts. Shared by every call site that runs
/// this same "validate args against the tool's schema" check --
/// `handler.rs`'s three `tools/call` pre-checks (which run ahead of
/// `Kind::call` for every kind, python included) and `kinds::python`'s own
/// `call`/`tool_run` (kept for direct `Kind::call` callers that bypass
/// `handler.rs`, e.g. the conformance suite) -- so every one of them
/// reports the same shape instead of `handler.rs`'s callers getting a bare
/// `e.to_string()` while python's direct callers got the full structure.
///
/// Names the failing top-level argument (the first JSON-pointer path
/// segment) and both types when the failure is a `type` mismatch; other
/// schema violations (pattern, enum, range, ...) still get `phase` +
/// `argument` but no `expected_type`/`actual_type`, since those keywords
/// don't name a single expected JSON type.
pub fn describe_args_error(err: &jsonschema::ValidationError<'_>) -> Value {
    let path = err.instance_path.as_str();
    let argument = path
        .trim_start_matches('/')
        .split('/')
        .next()
        .filter(|s| !s.is_empty());
    let mut data = json!({"phase": "args_coercion", "instance_path": path});
    let Some(obj) = data.as_object_mut() else {
        unreachable!("json!({{...}}) always builds an object")
    };
    if let Some(argument) = argument {
        obj.insert("argument".to_string(), json!(argument));
    }
    if let ValidationErrorKind::Type { kind } = &err.kind {
        let expected = match kind {
            TypeKind::Single(t) => t.to_string(),
            TypeKind::Multiple(bitmap) => (*bitmap)
                .into_iter()
                .map(|t| t.to_string())
                .collect::<Vec<_>>()
                .join(" or "),
        };
        obj.insert("expected_type".to_string(), json!(expected));
        obj.insert(
            "actual_type".to_string(),
            json!(json_type_name(&err.instance)),
        );
    }
    data
}

/// PRD-mcphost-tool-test AC7: `host.spec_test` refuses a request naming more
/// than this many example invocations, before any of them run.
pub const MAX_TEST_INVOCATIONS: usize = 5;

/// PRD-mcphost-tool-test AC2: a `host.spec_test` invocation's exception
/// traceback is bounded to this many bytes (tail-capped, same convention as
/// `python::TOOL_RUN_CAP_BYTES`) so a runaway recursive traceback can't blow
/// up the JSON-RPC response.
pub const TEST_TRACEBACK_CAP_BYTES: usize = 8 * 1024;

/// Turns a per-invocation [`KindError`] from [`Kind::call`] into
/// `host.spec_test`'s per-invocation error detail (PRD-mcphost-tool-test
/// AC2): exception class and a bounded traceback excerpt when the kind's
/// own error carries them (today only `python`, via
/// `KindError::Structured { code: "tool_exception", data, .. }` --
/// `kinds::python::map_envelope_error`), else just the error's own code and
/// message, so `echo`/`http` failures still report something rather than a
/// missing field.
pub fn describe_test_failure(err: &KindError) -> Value {
    match err {
        KindError::Structured {
            code,
            message,
            data,
        } => {
            let mut out = json!({"code": code, "message": message});
            if let Some(obj) = out.as_object_mut() {
                if let Some(exception_class) = data.get("exception_class") {
                    obj.insert("exception_class".to_string(), exception_class.clone());
                }
                if let Some(tb) = data.get("traceback").and_then(Value::as_str)
                    && !tb.is_empty()
                {
                    obj.insert(
                        "traceback".to_string(),
                        json!(python::cap_str_bytes(tb, TEST_TRACEBACK_CAP_BYTES)),
                    );
                }
            }
            out
        }
        KindError::InvalidArgs(m) => json!({"code": "invalid_args", "message": m}),
        KindError::InvalidSpec(m) => json!({"code": "invalid_spec", "message": m}),
        KindError::Exec(m) => json!({"code": "exec_error", "message": m}),
    }
}

/// `host.spec_test`'s shared execution core (PRD-mcphost-tool-test AC8): runs
/// each of `invocations` through the exact same [`Kind::call`] entry point a
/// published call dispatches to via `handler::call_published_tool` -- not a
/// second, weaker profile -- under the same per-call `timeout`. `ctx_factory`
/// builds a fresh [`CallCtx`] per invocation (so each gets its own deadline);
/// callers should set `test_mode: true` on it so a kind that renders a
/// request (`http`) echoes it back same as `host.tool_test`/`host.bridge_test`.
///
/// A per-invocation failure (an exception, an upstream error, a timeout) is
/// captured as `{"ok": false, ...}` in that invocation's own slot (AC2) --
/// it never aborts the remaining invocations, and this function itself never
/// returns an `Err`: the caller's overall JSON-RPC call succeeds regardless
/// of how many invocations failed.
pub async fn run_spec_test(
    kind: &Arc<dyn Kind>,
    spec: &Value,
    invocations: &[Value],
    timeout: Duration,
    mut ctx_factory: impl FnMut() -> CallCtx,
) -> Vec<Value> {
    let mut out = Vec::with_capacity(invocations.len());
    for args in invocations {
        let ctx = ctx_factory();
        let start = Instant::now();
        let outcome = tokio::time::timeout(timeout, kind.call(spec, args.clone(), &ctx)).await;
        let duration_ms = start.elapsed().as_millis() as i64;
        out.push(match outcome {
            Ok(Ok(output)) => json!({"ok": true, "output": output, "duration_ms": duration_ms}),
            Ok(Err(e)) => {
                let mut v = json!({"ok": false, "duration_ms": duration_ms});
                if let (Some(obj), Value::Object(err_fields)) =
                    (v.as_object_mut(), describe_test_failure(&e))
                {
                    obj.extend(err_fields);
                }
                v
            }
            Err(_elapsed) => json!({
                "ok": false,
                "duration_ms": duration_ms,
                "code": "call_timeout",
                "message": "invocation exceeded the call timeout",
            }),
        });
    }
    out
}

// ---- result envelope contract (PRD-mcphost-result-envelope-contract) ------
//
// A spec that declares `outputs` (field names its tool promises to emit)
// gets those fields promoted to `result.payload.<field>` regardless of kind
// or how deep the tool's own response nests them -- the class of defect the
// PRD's Problem statement measured: a working tool scored half-credit
// because its judge's gold check looked for a field at a path the tool
// never populated.

/// One level of common API-response wrapping a declared field is searched
/// under, beyond the source object's own top level (requirement 2, `http`).
const ENVELOPE_WRAPPER_KEYS: [&str; 3] = ["data", "result", "response"];

/// Finds `field` in `source`: at its top level, or nested one level inside
/// `wrapper_keys` -- `Some(&ENVELOPE_WRAPPER_KEYS)` for requirement 2's
/// `http` promotion (`data`/`result`/`response` only, an upstream REST
/// response's own common shapes), `None` for requirement 3's `python`
/// promotion (any single nested object key -- AC2's own example wraps
/// `diagnosis` in `analysis`, a key with no fixed name to enumerate, since
/// a python tool's return shape is the tool author's own code, not a
/// third-party API's). `None` (not found) when the field is at neither
/// depth.
fn find_declared_field<'a>(
    source: &'a Value,
    field: &str,
    wrapper_keys: Option<&[&str]>,
) -> Option<&'a Value> {
    let obj = source.as_object()?;
    if let Some(v) = obj.get(field) {
        return Some(v);
    }
    match wrapper_keys {
        Some(keys) => {
            for wrapper in keys {
                if let Some(v) = obj
                    .get(*wrapper)
                    .and_then(|w| w.as_object())
                    .and_then(|w| w.get(field))
                {
                    return Some(v);
                }
            }
        }
        None => {
            for wrapper_val in obj.values() {
                if let Some(v) = wrapper_val.as_object().and_then(|w| w.get(field)) {
                    return Some(v);
                }
            }
        }
    }
    None
}

/// Requirement 2, AC1: promotes every `declared` field found in `source` --
/// at its top level or nested one level inside `data`/`result`/`response`
/// -- into `payload`, skipping any field already present there (a tool that
/// already places a field correctly is unchanged -- Migration/compatibility:
/// additive, existing nested shapes stay). A no-op when `declared` is
/// empty, so a tool with no declared outputs sees no envelope changes at
/// all. Used by the `http` kind, whose body is a third-party upstream's own
/// response shape (see [`find_declared_field`]'s doc for why `python` uses
/// [`promote_declared_outputs_any_wrapper`] instead).
pub fn promote_declared_outputs(payload: &mut Map<String, Value>, source: &Value, declared: &[String]) {
    promote_declared_outputs_with(payload, source, declared, Some(&ENVELOPE_WRAPPER_KEYS));
}

/// Requirement 3, AC2: same as [`promote_declared_outputs`], but searches
/// one level deep under *any* key, not just `data`/`result`/`response` --
/// the `python` kind's tool code returns its own shape, so there is no
/// fixed set of "common wrapper" names to check.
pub fn promote_declared_outputs_any_wrapper(
    payload: &mut Map<String, Value>,
    source: &Value,
    declared: &[String],
) {
    promote_declared_outputs_with(payload, source, declared, None);
}

fn promote_declared_outputs_with(
    payload: &mut Map<String, Value>,
    source: &Value,
    declared: &[String],
    wrapper_keys: Option<&[&str]>,
) {
    for field in declared {
        if payload.contains_key(field) {
            continue;
        }
        if let Some(v) = find_declared_field(source, field, wrapper_keys) {
            payload.insert(field.clone(), v.clone());
        }
    }
}

/// Requirement 4: `host.tool_test`'s per-declared-field report -- which
/// fields the contract path (`result.payload.<field>`) actually carries
/// after a real call, and which are missing (the defect that docks a
/// caller's judge half-credit, per the Problem statement). `payload` is
/// whatever [`Kind::payload_from_call_result`] located, `None` when this
/// kind's result had no `payload` at all. Returns `None` when `declared` is
/// empty -- a tool with no declared outputs gets no envelope section.
pub fn envelope_report(declared: &[String], payload: Option<&Value>) -> Option<Value> {
    if declared.is_empty() {
        return None;
    }
    let mut found = Vec::new();
    let mut missing = Vec::new();
    for field in declared {
        let present = payload.and_then(|p| p.as_object()).is_some_and(|p| p.contains_key(field));
        if present {
            found.push(field.clone());
        } else {
            missing.push(field.clone());
        }
    }
    let green = missing.is_empty();
    Some(json!({
        "declared": declared,
        "found_at_contract_path": found,
        "missing": missing,
        "green": green,
    }))
}

/// What a `Kind::describe` call reports about the tool it would publish.
#[derive(Debug, Clone)]
pub struct ToolDescriptor {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}

/// A complete, minimal, working example for this kind (PRD-mcphost-publish-first-try
/// requirement 1 / 4): a `spec` that `host.tool_publish` accepts as-is, `call_args`
/// that satisfy the resulting tool's own `describe`d schema, and a one-sentence
/// `blurb` naming which fields are optional and why. `host.tool_publish`'s wire
/// description (`handler::host_tools`) and `host.quickstart` are both built from
/// this single source per kind, so they cannot drift from each other -- though
/// (see the PRD's requirement 6, not attempted this tick) the crate's README is
/// still hand-maintained separately, not rendered from this same source.
#[derive(Debug, Clone)]
pub struct KindExample {
    pub spec: Value,
    pub call_args: Value,
    /// Owned, not `&'static str` (2026-09-04, requirement 6 / AC6): every
    /// registered kind's real `example()` now derives this from its own
    /// `docs/kinds/<name>.md` file via [`docs::parse_kind_doc`] rather than
    /// a hand-duplicated literal, so the crate's README and the on-wire
    /// description can't drift from each other -- see the `docs` module.
    pub blurb: String,
}

/// Resolves one of the calling tenant's stored secrets by name.
pub trait SecretResolver: Send + Sync {
    fn resolve(&self, name: &str) -> Option<String>;
}

/// A resolver with no secrets, for contexts (like the conformance suite)
/// that don't need one.
pub struct NoSecrets;
impl SecretResolver for NoSecrets {
    fn resolve(&self, _name: &str) -> Option<String> {
        None
    }
}

/// Handle a `Kind::call` uses to append lines to that call's log, later
/// readable via `host.tool_logs`.
pub trait CallLog: Send + Sync {
    fn log(&self, line: &str);
}

/// A call log that discards everything, for contexts that don't need one.
pub struct NullLog;
impl CallLog for NullLog {
    fn log(&self, _line: &str) {}
}

/// Sink a [`Kind::call`] reports a sandboxed subprocess's resource usage to
/// (PRD-mcphost-code-tools requirement 8: the `calls` row records CPU time
/// and peak memory). A `Kind` with no subprocess to meter (`echo`, `http`)
/// never calls this; `handler.rs` reads back whatever was recorded (if
/// anything) after `Kind::call` returns and writes it into the `calls` row
/// alongside the rest of the call's metering.
pub trait ResourceSink: Send + Sync {
    fn record(&self, cpu_ms: i64, peak_rss_kb: i64);
}

/// A resource sink that discards everything, for contexts (tests, kinds
/// with no subprocess) that don't need one.
pub struct NullResourceSink;
impl ResourceSink for NullResourceSink {
    fn record(&self, _cpu_ms: i64, _peak_rss_kb: i64) {}
}

/// Context passed to every `Kind::call`: who is calling, how to reach their
/// secrets, when to give up, and where to log.
pub struct CallCtx {
    pub tenant_id: i64,
    pub namespace: String,
    pub secrets: Arc<dyn SecretResolver>,
    pub deadline: Instant,
    pub log: Arc<dyn CallLog>,
    /// Set by `host.tool_test` (P1 requirement 9): the call still hits the
    /// real upstream, but a `Kind` that supports test mode should include
    /// the rendered request (secrets redacted) in its result. Kinds that
    /// don't have a notion of a "rendered request" (e.g. `echo`) ignore
    /// this; it defaults to `false` for every ordinary call.
    pub test_mode: bool,
    /// See [`ResourceSink`]. Defaults to [`NullResourceSink`] everywhere but
    /// `handler.rs`'s real dispatch path.
    pub resources: Arc<dyn ResourceSink>,
    /// PRD-mcphost-code-tools-warm-pool: the tool's own local (unqualified)
    /// name, when the caller (`handler.rs`) already knows it -- i.e. every
    /// real dispatch path (`<namespace>.<name>`, `host.tool_call`,
    /// `host.tool_test`, `host.tool_run`). `Kind::call`'s signature has
    /// never carried the tool's name (see `kinds::python`'s own module docs
    /// on why its env directories can't be keyed by it either); this field
    /// is the additive fix, needed so a `Kind` that keeps per-tool
    /// out-of-process state (a warm sandbox pool) can key and evict it.
    /// `None` in every context that has no such name (`for_test`, the
    /// conformance suite) -- a `Kind` that needs it degrades to "always
    /// cold" rather than panicking when it's absent.
    pub tool_name: Option<String>,
}

impl CallCtx {
    /// Convenience constructor for tests and the conformance suite.
    pub fn for_test(tenant_id: i64, namespace: impl Into<String>) -> Self {
        Self {
            tenant_id,
            namespace: namespace.into(),
            secrets: Arc::new(NoSecrets),
            deadline: Instant::now() + Duration::from_secs(30),
            log: Arc::new(NullLog),
            test_mode: false,
            resources: Arc::new(NullResourceSink),
            tool_name: None,
        }
    }

    pub fn time_remaining(&self) -> Duration {
        self.deadline.saturating_duration_since(Instant::now())
    }
}

/// A tool execution kind. Implementors are registered in a [`KindRegistry`]
/// under a fixed [`Kind::name`]; `host.tool_publish` names the kind by that
/// string and stores the caller's `spec` alongside it.
#[async_trait::async_trait]
pub trait Kind: Send + Sync {
    /// The registry key this kind is published under (e.g. `"echo"`).
    fn name(&self) -> &'static str;

    /// Validate a tenant-supplied `spec` before it is stored. Return
    /// [`KindError::InvalidSpec`] naming what's wrong.
    fn validate(&self, spec: &Value) -> Result<(), KindError>;

    /// Every simultaneously-failing field, not just the first (requirement 3
    /// / AC2): a `Kind` that can cheaply check more than one field
    /// independently should override this to collect every violation
    /// instead of returning at the first with `?`, so `host.tool_publish`
    /// reports every failing field in one round trip instead of one per
    /// attempt. Defaults to running [`Kind::validate`] and wrapping its
    /// single `Err` in a one-element vec (empty on `Ok`) -- every existing
    /// `Kind` gets this for free with no behavior change until it opts in.
    fn validate_all(&self, spec: &Value) -> Vec<KindError> {
        match self.validate(spec) {
            Ok(()) => Vec::new(),
            Err(e) => vec![e],
        }
    }

    /// Deeper validation that needs to run out-of-process (PRD-mcphost-code-tools
    /// requirement 2: a `python` tool's source must be parsed and checked
    /// for `main` "in the sandbox, never in the host process", which means
    /// a subprocess call -- something [`Kind::validate`]'s synchronous
    /// signature can't do). Runs after `validate` succeeds, still before the
    /// spec is stored. Kinds with no such need (`echo`, `http`) use the
    /// default no-op.
    async fn validate_async(&self, _spec: &Value) -> Result<(), KindError> {
        Ok(())
    }

    /// Describe the tool this `spec` would publish: its (kind-local) name,
    /// description, and JSON Schema for arguments. The host overrides
    /// `ToolDescriptor::name` with `<namespace>.<published-name>` when it
    /// builds the wire-level `Tool`; what this method returns for `name` is
    /// only ever used as a hint (e.g. in error messages).
    fn describe(&self, spec: &Value) -> ToolDescriptor;

    /// Names of `secret.<name>` references this `spec` would need at call
    /// time, so `host.tool_publish` can reject an unknown secret before the
    /// tool is ever called (requirement 3 / AC3: `secret_missing` naming
    /// the secret). Kinds with no secret-templating concept (`echo`) don't
    /// override this; the default is "none needed."
    fn referenced_secrets(&self, _spec: &Value) -> Vec<String> {
        Vec::new()
    }

    /// PRD-mcphost-tool-test AC1: the pip requirements a publish of this
    /// `spec` would build its environment with -- reported by
    /// `host.spec_test` alongside `describe`'s `input_schema` so a
    /// pre-publish dry run sees both halves of what publishing would infer.
    /// `Vec::new()` (the default) for a kind with no such notion (`echo`,
    /// `http`); only `python` overrides it.
    fn requirements(&self, _spec: &Value) -> Vec<String> {
        Vec::new()
    }

    /// A complete minimal working example for this kind (see
    /// [`KindExample`]), used to build `host.tool_publish`'s per-kind
    /// description and `host.quickstart`'s filled-in call sequence.
    /// Defaults to an empty placeholder so test-only `Kind` impls (e.g.
    /// `tests/ac15_call_timeout.rs`'s never-completing kind,
    /// `tests/ac17_kind_conformance.rs`'s conformance fixtures) don't have
    /// to implement it; every kind actually registered in `main.rs`
    /// (`echo`, `http`, `python`) overrides it.
    fn example(&self) -> KindExample {
        KindExample {
            spec: Value::Null,
            call_args: serde_json::json!({}),
            blurb: String::new(),
        }
    }

    /// Execute a call. `args` have not yet been validated against the
    /// descriptor's input schema when this is invoked directly (the host's
    /// dispatch path validates first); implementations that are exercised
    /// directly by the conformance suite should still validate defensively.
    async fn call(&self, spec: &Value, args: Value, ctx: &CallCtx) -> Result<Value, KindError>;

    /// PRD-mcphost-code-tools-warm-pool requirement 3: `host.tool_run` --
    /// the same execution `call` would do, minus a `calls` row and metering,
    /// returning whatever raw debug information (stdout, stderr, exit code,
    /// ...) this kind can produce. Kinds with no such notion (`echo`,
    /// `http`) don't override this; the default names the RPC unsupported
    /// for that kind rather than silently falling back to an ordinary call
    /// (which would defeat the "no calls row" guarantee `handler.rs` can't
    /// itself enforce for a kind it doesn't understand).
    async fn tool_run(
        &self,
        _spec: &Value,
        _args: Value,
        _ctx: &CallCtx,
    ) -> Result<Value, KindError> {
        Err(KindError::structured(
            "tool_run_unsupported",
            "this tool's kind does not support host.tool_run",
        ))
    }

    /// A previously-published tool under this kind is gone -- republished
    /// (new source/spec under the same name) or removed outright. Kinds
    /// with no out-of-process state tied to a specific tool name (`echo`,
    /// `http`) don't override this. `kinds::python` overrides it to kill and
    /// evict any warm sandbox for `(tenant_id, local_name)` synchronously,
    /// so PRD-mcphost-code-tools-warm-pool AC3's "killed before the
    /// operation returns" holds regardless of the warm sandbox's TTL.
    async fn on_tool_changed(&self, _tenant_id: i64, _local_name: &str) {}

    /// A tenant is gone (disabled or deleted). Same idea as
    /// [`Kind::on_tool_changed`], but for every tool of this kind that
    /// tenant owns at once -- `kinds::python` evicts every warm sandbox
    /// keyed to `tenant_id`, regardless of tool name.
    async fn on_tenant_removed(&self, _tenant_id: i64) {}

    /// PRD-mcphost-sandbox-ready requirement 1/3: this kind's most recent
    /// sandbox self-test result, if it has one. `None` (the default) for a
    /// kind with no sandboxed-subprocess concept (`echo`, `http`) -- neither
    /// `/healthz` nor `host.tool_publish`'s readiness gate treat `None` as
    /// "unready", only an explicit `Some(status)` with `ready: false` does,
    /// so those kinds are entirely unaffected (PRD non-goal / AC4).
    /// `/healthz` and the publish-time gate both call this through
    /// [`KindRegistry::all`] rather than a concrete downcast, so a future
    /// second sandboxed kind (e.g. `wasm`) gets the same reporting for free.
    fn sandbox_status(&self) -> Option<crate::sandbox::SandboxStatus> {
        None
    }

    /// PRD-mcphost-sandbox-ready requirement 4: re-runs this kind's sandbox
    /// self-test immediately (`admin.sandbox_recheck`) and returns the
    /// fresh status, having already updated whatever [`Kind::sandbox_status`]
    /// reads from. `None` (the default) for a kind with none.
    async fn sandbox_recheck(&self) -> Option<crate::sandbox::SandboxStatus> {
        None
    }

    /// PRD-mcphost-result-envelope-contract requirement 1: the output field
    /// names this `spec` declares (its own `outputs`, when present) --
    /// `Kind::call` promotes each to `result.payload.<field>` and
    /// `host.tool_test` reports any that never land there. `Vec::new()`
    /// (the default) for a kind with no declared-outputs concept, or an
    /// unparseable spec (publish-time validation already rejects that spec
    /// before this is ever reached in practice; this fallback exists only
    /// so the method never panics) -- a tool that declares nothing gets no
    /// envelope changes at all (additive, see Migration/compatibility).
    fn declared_outputs(&self, _spec: &Value) -> Vec<String> {
        Vec::new()
    }

    /// PRD-mcphost-result-envelope-contract requirement 4: locates the
    /// envelope contract's `payload` object inside whatever `Value`
    /// `Kind::call` returned, so `host.tool_test` can check declared
    /// fields against it without knowing each kind's own test-mode
    /// wrapping. The default covers an ordinary (non-test-mode) call,
    /// where `payload` sits at the result's own top level; a kind whose
    /// `test_mode` echo nests the response elsewhere (`http`'s
    /// `response.payload`, `python`'s `result.payload`) overrides this to
    /// also check that path.
    fn payload_from_call_result<'a>(&self, call_result: &'a Value) -> Option<&'a Value> {
        call_result.get("payload")
    }
}

/// Registry of known [`Kind`]s, keyed by [`Kind::name`]. `host.tool_publish`
/// looks kinds up here; an unregistered kind fails naming the registered
/// set.
#[derive(Clone, Default)]
pub struct KindRegistry {
    kinds: BTreeMap<&'static str, Arc<dyn Kind>>,
}

impl KindRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// The registry shipped by this crate: `echo` only. Feature-PRD crates
    /// extend this by calling [`KindRegistry::register`] with their own
    /// kinds after constructing this base.
    pub fn with_builtin() -> Self {
        let mut registry = Self::new();
        registry.register(Arc::new(echo::EchoKind));
        registry
    }

    pub fn register(&mut self, kind: Arc<dyn Kind>) -> &mut Self {
        self.kinds.insert(kind.name(), kind);
        self
    }

    pub fn get(&self, name: &str) -> Option<Arc<dyn Kind>> {
        self.kinds.get(name).cloned()
    }

    /// Registered kind names, stable order, for error messages.
    pub fn names(&self) -> Vec<&'static str> {
        self.kinds.keys().copied().collect()
    }

    /// Every registered kind, for a lifecycle notification
    /// (`on_tool_changed`/`on_tenant_removed`) that must reach whichever
    /// kind actually owns the affected tool -- `control.rs`/`admin.rs` don't
    /// know which kind that is at the call site (a republish's row may have
    /// just been overwritten, and a tenant delete's cascade doesn't name
    /// kinds at all), so both simply notify every kind and let each decide
    /// whether it has any state to clean up (the default no-op costs
    /// nothing for `echo`/`http`).
    pub fn all(&self) -> impl Iterator<Item = &Arc<dyn Kind>> {
        self.kinds.values()
    }
}
