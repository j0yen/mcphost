//! Tool execution kinds: the `Kind` trait and its registry.
//!
//! `mcphost` publishes tools of a *kind* (`echo` is the reference
//! implementation shipped here; REST-wrapper and code kinds are separate
//! PRDs that extend this crate's registry via `build_into`). A `Kind` knows
//! how to validate a tenant-supplied `spec`, describe the tool it produces
//! for `tools/list`, and execute a call against it.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use jsonschema::error::{TypeKind, ValidationErrorKind};
use serde_json::{Map, Value, json};

pub mod chain;
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
    /// this crate's generic `invalid_spec`/`args_invalid`/`internal`
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

/// JSON type name for a [`Value`] in plain JSON's own six-type vocabulary
/// (PRD-mcphost-spec-output-paths requirement 1's `got`) -- unlike
/// [`json_type_name`] above, every number is just `"number"`; JSON itself
/// has no separate "integer" type (that's a JSON-*Schema* distinction this
/// PRD's structured `invalid_spec.got` deliberately doesn't make, since
/// AC2's example -- `outputs.a: 5` -- names `got` as `"number"`, not
/// `"integer"`).
fn plain_json_type_name(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
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
        KindError::InvalidArgs(m) => json!({"code": "args_invalid", "message": m}),
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
                "message": format!(
                    "invocation exceeded the {}s deadline",
                    timeout.as_secs()
                ),
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
pub(crate) const ENVELOPE_WRAPPER_KEYS: [&str; 3] = ["data", "result", "response"];

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
///
/// `missing` itself stays the plain list of field names it always was
/// (PRD-mcphost-spec-output-paths requirement 9: every existing
/// `tests/envelope_ac*.rs` assertion on that shape keeps passing unchanged).
/// The same PRD's requirement 5 -- telling the publisher *where* a missing
/// field was actually seen -- is additive: `missing_detail` carries one
/// entry per missing field, `{"name", "path"}` when that entry declared an
/// explicit path (the path that was tried and came up empty), or
/// `{"name", "seen_at"}` (up to three candidate paths, omitted entirely when
/// none are found) for a bare-name entry, searched via
/// [`find_seen_at`] against `seen_at_source` -- the un-promoted body
/// ([`Kind::source_for_output_search`]), not `payload`, so a hint like
/// `$.json.ingestion_status` isn't itself dulled by the same wrapper-search
/// depth limit that missed the field in the first place.
pub fn envelope_report(
    declared: &[OutputDecl],
    payload: Option<&Value>,
    seen_at_source: Option<&Value>,
) -> Option<Value> {
    if declared.is_empty() {
        return None;
    }
    let mut found = Vec::new();
    let mut missing = Vec::new();
    let mut missing_detail = Vec::new();
    for decl in declared {
        let present = payload
            .and_then(|p| p.as_object())
            .is_some_and(|p| p.contains_key(&decl.name));
        if present {
            found.push(decl.name.clone());
        } else {
            missing.push(decl.name.clone());
            let mut detail = json!({"name": decl.name});
            if let Some(obj) = detail.as_object_mut() {
                match &decl.path {
                    Some(path) => {
                        obj.insert("path".to_string(), json!(path.as_str()));
                    }
                    None => {
                        if let Some(source) = seen_at_source {
                            let hints = find_seen_at(source, &decl.name, 3, 3);
                            if !hints.is_empty() {
                                obj.insert("seen_at".to_string(), json!(hints));
                            }
                        }
                    }
                }
            }
            missing_detail.push(detail);
        }
    }
    let green = missing.is_empty();
    Some(json!({
        "declared": declared.iter().map(|d| d.name.clone()).collect::<Vec<_>>(),
        "found_at_contract_path": found,
        "missing": missing,
        "missing_detail": missing_detail,
        "green": green,
    }))
}

// ---- declared-output paths (PRD-mcphost-spec-output-paths) ----------------
//
// `outputs` may name a plain field (promoted via the wrapper search above,
// unchanged -- non-goal 2) or pair a field with a `Path` saying exactly
// where to read it from the upstream/return body, closing the class of
// defect where a declared field sits one key deeper than the wrapper search
// looks (the map form the Problem statement's recorded sessions reached for
// on their own).

/// One `outputs` entry, normalized from either wire form (a bare name in a
/// list, or a name/path pair in a map) by [`normalize_outputs`]. `path` is
/// `None` for a list entry -- it keeps today's wrapper-search promotion
/// (requirement 2); `Some` for a map entry -- requirement 4 reads exactly
/// that path from the source body instead of searching.
#[derive(Debug, Clone, PartialEq)]
pub struct OutputDecl {
    pub name: String,
    pub path: Option<Path>,
}

/// One segment of a [`Path`]: a dotted object key or a bracketed array
/// index.
#[derive(Debug, Clone, PartialEq)]
enum PathSegment {
    Key(String),
    Index(usize),
}

/// A parsed `$.a.b[0].c`-style path (requirement 3): `$` followed by zero or
/// more `.<key>` / `[<uint>]` segments. Deliberately not a general JSONPath
/// engine (non-goal 1) -- [`Path::parse`] rejects a wildcard, filter, or
/// recursive-descent expression by name rather than silently misreading it.
#[derive(Debug, Clone, PartialEq)]
pub struct Path {
    raw: String,
    segments: Vec<PathSegment>,
}

/// Why a candidate path string was rejected (requirement 3 / AC8): named
/// separately from a generic "malformed" so the error message can say
/// exactly which unsupported JSONPath feature was used.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PathParseError {
    Wildcard,
    Filter,
    RecursiveDescent,
    Malformed,
}

impl PathParseError {
    pub(crate) fn reason(self) -> &'static str {
        match self {
            PathParseError::Wildcard => "uses a wildcard",
            PathParseError::Filter => "uses a filter expression",
            PathParseError::RecursiveDescent => "uses recursive descent",
            PathParseError::Malformed => "is not a valid path",
        }
    }
}

impl Path {
    /// Requirement 3: parses `raw` as `$` followed by zero or more
    /// `.<key>` / `[<uint>]` segments. A key is restricted to
    /// alphanumeric/`_`/`-` (every recorded fixture's field names, and every
    /// JSON object key this crate's own kinds produce, fit that set); a
    /// bracketed segment must be a non-negative integer. Anything else --
    /// `[*]`, `.*`, `[?...]`, `..` (recursive descent), or a string that
    /// doesn't start with `$` -- is rejected with the specific reason AC8
    /// wants named.
    /// PRD-mcphost-composition requirement 4: `chain`'s step-argument
    /// mapping paths (`$.prev.result.payload.x`, `$.steps[i]...`,
    /// `$.input.x`) use this exact grammar, so `chain.rs` parses them with
    /// this same `Path::parse` rather than a second implementation.
    /// `pub(crate)` rather than private for that reason.
    pub(crate) fn parse(raw: &str) -> Result<Path, PathParseError> {
        let rest = raw.strip_prefix('$').ok_or(PathParseError::Malformed)?;
        if rest.contains("[*]") || rest.contains(".*") {
            return Err(PathParseError::Wildcard);
        }
        if rest.contains("[?") {
            return Err(PathParseError::Filter);
        }
        if rest.contains("..") {
            return Err(PathParseError::RecursiveDescent);
        }
        let bytes = rest.as_bytes();
        let mut i = 0;
        let mut segments = Vec::new();
        while i < bytes.len() {
            match bytes[i] {
                b'.' => {
                    i += 1;
                    let start = i;
                    while i < bytes.len() && bytes[i] != b'.' && bytes[i] != b'[' {
                        i += 1;
                    }
                    let key = &rest[start..i];
                    if key.is_empty()
                        || !key
                            .chars()
                            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
                    {
                        return Err(PathParseError::Malformed);
                    }
                    segments.push(PathSegment::Key(key.to_string()));
                }
                b'[' => {
                    i += 1;
                    let start = i;
                    while i < bytes.len() && bytes[i] != b']' {
                        i += 1;
                    }
                    if i >= bytes.len() {
                        return Err(PathParseError::Malformed);
                    }
                    let idx: usize = rest[start..i]
                        .parse()
                        .map_err(|_| PathParseError::Malformed)?;
                    segments.push(PathSegment::Index(idx));
                    i += 1;
                }
                _ => return Err(PathParseError::Malformed),
            }
        }
        Ok(Path {
            raw: raw.to_string(),
            segments,
        })
    }

    pub fn as_str(&self) -> &str {
        &self.raw
    }

    /// Requirement 4: walks `source` by this path's segments, returning
    /// `None` (rather than an error) the moment a segment doesn't resolve --
    /// the caller (http's `call`, [`envelope_report`]'s `missing` detail)
    /// treats "not found" as the ordinary, expected outcome of a path the
    /// upstream didn't populate this time.
    pub fn resolve<'a>(&self, source: &'a Value) -> Option<&'a Value> {
        let mut cur = source;
        for seg in &self.segments {
            cur = match seg {
                PathSegment::Key(k) => cur.as_object()?.get(k)?,
                PathSegment::Index(i) => cur.as_array()?.get(*i)?,
            };
        }
        Some(cur)
    }
}

/// Requirement 1's structured `invalid_spec` shape: `field` (a dotted path),
/// `got` (the JSON type found there), `expected` (a plain phrase), `example`
/// (one accepted value) -- message `invalid spec: <field>: expected
/// <expected>, got <got>; e.g. <example>`. Used both by [`spec_parse_error`]
/// (a whole-struct deserialize failure) and [`normalize_outputs`] (an
/// `outputs` entry with the wrong shape).
fn invalid_spec_error(field: impl Into<String>, got: &str, expected: &str, example: Value) -> KindError {
    let field = field.into();
    let message = format!(
        "invalid spec: {field}: expected {expected}, got {got}; e.g. {}",
        serde_json::to_string(&example).unwrap_or_default()
    );
    KindError::structured_with(
        "invalid_spec",
        message,
        json!({"field": field, "got": got, "expected": expected, "example": example}),
    )
}

/// Same shape as [`invalid_spec_error`], but for requirement 3's path-syntax
/// rejections, whose message follows the "path "..." uses a wildcard;
/// supported: ..." phrasing the PRD's own user story spells out verbatim,
/// rather than the generic "expected X, got Y" template (there is no
/// meaningful "got" type distinct from "string" once a field has already
/// passed [`normalize_outputs`]'s own type check).
fn invalid_path_error(field: impl Into<String>, raw_path: &str, reason: &'static str) -> KindError {
    let field = field.into();
    let expected = "dotted keys and integer indices, e.g. \"$.items[0].ok\"";
    let example = json!("$.items[0].ok");
    let message = format!(
        "invalid spec: {field}: path \"{raw_path}\" {reason}; supported: {expected}"
    );
    KindError::structured_with(
        "invalid_spec",
        message,
        json!({"field": field, "got": "string", "expected": expected, "example": example}),
    )
}

/// Requirement 2/3: turns a raw `outputs` [`Value`] (as received on the
/// wire, before this function nothing has checked its shape beyond "valid
/// JSON") into normalized [`OutputDecl`]s. Accepts either a list of field
/// name strings (today's only form, unchanged -- entries get `path: None`)
/// or an object mapping each field name to a path string (requirement 2).
/// Every other shape -- a list entry that isn't a string, a map value that
/// isn't a string, or the top-level value being neither a list nor an
/// object -- is rejected with [`invalid_spec_error`]; a path string that
/// doesn't parse is rejected with [`invalid_path_error`] naming
/// `outputs.<name>` (requirement 3).
pub fn normalize_outputs(value: &Value) -> Result<Vec<OutputDecl>, KindError> {
    match value {
        Value::Array(items) => {
            let mut out = Vec::with_capacity(items.len());
            for (i, item) in items.iter().enumerate() {
                match item.as_str() {
                    Some(name) => out.push(OutputDecl {
                        name: name.to_string(),
                        path: None,
                    }),
                    None => {
                        return Err(invalid_spec_error(
                            format!("outputs[{i}]"),
                            plain_json_type_name(item),
                            "a field name string",
                            json!("status"),
                        ));
                    }
                }
            }
            Ok(out)
        }
        Value::Object(map) => {
            let mut out = Vec::with_capacity(map.len());
            for (name, path_val) in map {
                let field = format!("outputs.{name}");
                let raw_path = path_val.as_str().ok_or_else(|| {
                    invalid_spec_error(
                        field.clone(),
                        plain_json_type_name(path_val),
                        "a path string (dotted keys and integer indices, e.g. \"$.a.b[0].c\")",
                        json!("$.json.a"),
                    )
                })?;
                let path = Path::parse(raw_path)
                    .map_err(|e| invalid_path_error(field.clone(), raw_path, e.reason()))?;
                out.push(OutputDecl {
                    name: name.clone(),
                    path: Some(path),
                });
            }
            Ok(out)
        }
        other => Err(invalid_spec_error(
            "outputs",
            plain_json_type_name(other),
            "a list of field names or an object mapping field names to path strings",
            json!({"status": "$.data.status"}),
        )),
    }
}

/// Requirement 1: turns a whole-spec-struct deserialize failure (run through
/// `serde_path_to_error` at each kind's `parse_spec`) into the same
/// structured `invalid_spec` shape [`invalid_spec_error`] builds by hand --
/// `field` is the dotted path `serde_path_to_error` names, `got` is looked
/// up by walking `raw_spec` (the original, still-untyped JSON) along that
/// same path, and `expected`/`example` come from `field_hint`, the calling
/// kind's own table of its raw struct's field names (an unlisted/nested
/// field -- there are none in either kind's flat raw struct today -- falls
/// back to a generic phrase built from serde's own error detail, so this
/// never panics or silently drops information).
pub fn spec_parse_error<E: std::fmt::Display>(
    raw_spec: &Value,
    err: serde_path_to_error::Error<E>,
    field_hint: impl Fn(&str) -> Option<(&'static str, Value)>,
) -> KindError {
    let field = err.path().to_string();
    let field = if field.is_empty() { "spec".to_string() } else { field };
    let got = value_at_dotted_path(raw_spec, &field)
        .map(plain_json_type_name)
        .unwrap_or("missing");
    match field_hint(&field) {
        Some((expected, example)) => invalid_spec_error(field, got, expected, example),
        None => {
            let inner = err.into_inner();
            KindError::InvalidSpec(format!("{field}: {inner}"))
        }
    }
}

/// Walks `root` along a `serde_path_to_error`-formatted dotted/bracketed
/// path (`"a.b[0].c"`, the same syntax [`Path`] parses, minus the leading
/// `$`) to find the JSON value actually present there -- used only to
/// compute [`spec_parse_error`]'s `got` type, so a lookup miss (the field is
/// simply absent, distinct from present-with-the-wrong-type) is `None`
/// rather than an error.
fn value_at_dotted_path<'a>(root: &'a Value, path: &str) -> Option<&'a Value> {
    fn flush_key<'a>(cur: &'a Value, key: &str) -> Option<&'a Value> {
        if key.is_empty() {
            Some(cur)
        } else {
            cur.as_object()?.get(key)
        }
    }

    let mut cur = root;
    let mut pending_key = String::new();
    let mut chars = path.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        match c {
            '.' => {
                cur = flush_key(cur, &pending_key)?;
                pending_key.clear();
            }
            '[' => {
                cur = flush_key(cur, &pending_key)?;
                pending_key.clear();
                let start = i + 1;
                let mut end = start;
                for (j, c2) in chars.by_ref() {
                    if c2 == ']' {
                        end = j;
                        break;
                    }
                }
                let idx: usize = path.get(start..end)?.parse().ok()?;
                cur = cur.as_array()?.get(idx)?;
            }
            _ => pending_key.push(c),
        }
    }
    flush_key(cur, &pending_key)
}

/// Requirement 4: like [`promote_declared_outputs_with`], but per-declared
/// entry rather than per plain name -- an [`OutputDecl`] with a `path`
/// reads exactly that path from `source` (leaving the field absent, not an
/// error, when the path doesn't resolve); one with no path keeps the
/// existing wrapper-search promotion (`wrapper_keys`), unchanged (non-goal
/// 2).
///
/// A declared `path` is authoritative and is resolved (and, on a miss,
/// removed) unconditionally -- never skipped because `payload` already
/// carries a same-named key. `payload` starts as a clone of the raw body
/// (the "payload mirrors body" shape from PRD-mcphost-result-envelope-
/// contract), so a body whose top level happens to already have a key
/// named `bridge_status` must not let that native value silently win over
/// a declared `outputs: {"bridge_status": "$.json.bridge_status"}`; AC1/
/// AC4/AC5 all describe the path's own resolution, not whatever the body
/// already put there. The pre-existing-key skip stays for path-less
/// entries, which fall through to `promote_declared_outputs_with`'s own
/// (unchanged, non-goal 2) skip-if-present wrapper search.
pub fn apply_output_decls(
    payload: &mut Map<String, Value>,
    source: &Value,
    declared: &[OutputDecl],
    wrapper_keys: Option<&[&str]>,
) {
    let mut no_path_names = Vec::new();
    for decl in declared {
        match &decl.path {
            Some(path) => match path.resolve(source) {
                Some(v) => {
                    payload.insert(decl.name.clone(), v.clone());
                }
                None => {
                    payload.remove(&decl.name);
                }
            },
            None => {
                if !payload.contains_key(&decl.name) {
                    no_path_names.push(decl.name.clone());
                }
            }
        }
    }
    promote_declared_outputs_with(payload, source, &no_path_names, wrapper_keys);
}

/// Requirement 5: up to `max_results` dotted paths (`$.a.b`) where a key
/// named `field` exists anywhere in `source`, searched breadth-first by
/// depth (top-level keys are depth 1) up to `max_depth` -- so
/// `host.tool_test`'s report can tell a publisher who declared a bare name
/// exactly where in the body a same-named key actually sits, instead of
/// leaving them to guess (the defect AC6 names: a list-form `outputs` entry
/// whose value sits one key deeper than the wrapper search looks).
fn find_seen_at(source: &Value, field: &str, max_depth: usize, max_results: usize) -> Vec<String> {
    let mut out = Vec::new();
    let mut path = Vec::new();
    find_seen_at_walk(source, field, &mut path, 1, max_depth, max_results, &mut out);
    out
}

#[allow(clippy::too_many_arguments)]
fn find_seen_at_walk(
    value: &Value,
    field: &str,
    path: &mut Vec<String>,
    depth: usize,
    max_depth: usize,
    max_results: usize,
    out: &mut Vec<String>,
) {
    let Value::Object(map) = value else {
        return;
    };
    for (key, val) in map {
        if out.len() >= max_results {
            return;
        }
        path.push(key.clone());
        if key == field {
            out.push(format!("$.{}", path.join(".")));
        }
        if out.len() < max_results && depth < max_depth {
            find_seen_at_walk(val, field, path, depth + 1, max_depth, max_results, out);
        }
        path.pop();
    }
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

/// A backend a stateful `Kind`'s sandboxed call can use for the tenant's
/// `host.state.*` store (PRD-mcphost-tenant-state requirement 3: `mcphost.
/// state` inside the python kind's sandbox). `op` names one of
/// `tenant_state.rs`'s own verbs (`"get"`, `"set"`, `"delete"`, `"list"`,
/// `"table_create"`, `"table_drop"`, `"insert"`, `"query"`,
/// `"delete_rows"`) and `args` is that verb's own JSON argument object --
/// the exact shapes `tenant_state::state_get`/`state_set`/etc. already
/// take, so `handler.rs`'s implementation is a one-line dispatch, not a
/// translation layer.
#[async_trait::async_trait]
pub trait StateBackend: Send + Sync {
    async fn call(&self, op: &str, args: Value) -> Result<Value, KindError>;
}

/// A backend with no store behind it, for contexts (tests, the conformance
/// suite, and any `CallCtx` that hasn't wired a tenant's real state in) that
/// don't need one. Every op fails structured, naming itself unavailable
/// rather than silently no-op'ing -- a tool relying on `mcphost.state` in
/// one of these contexts should see a clear error, not state that quietly
/// never persists.
pub struct NoState;
#[async_trait::async_trait]
impl StateBackend for NoState {
    async fn call(&self, _op: &str, _args: Value) -> Result<Value, KindError> {
        Err(KindError::structured(
            "state_unavailable",
            "no tenant state backend is wired for this call context",
        ))
    }
}

/// PRD-mcphost-runs-and-jobs P0 requirement 5: where a sandboxed call's
/// `mcphost.progress(pct, msg)` (see `kinds::python`'s `ProgressSidecarBridge`)
/// lands. `handler.rs`'s real dispatch path wires this to a sink that
/// writes into the `runs` row this call is executing as (job triggers
/// only); every other context (an ordinary synchronous call, tests, the
/// conformance suite) gets [`NullProgress`], so a tool calling
/// `mcphost.progress` outside a job context is a harmless no-op rather than
/// an error -- unlike [`StateBackend`], there is no "progress unavailable"
/// failure mode a tool's own code needs to handle.
pub trait ProgressSink: Send + Sync {
    fn report(&self, pct: Option<i64>, msg: Option<String>);
}

/// A progress sink that discards everything -- every `CallCtx` not
/// executing as a job gets this.
pub struct NullProgress;
impl ProgressSink for NullProgress {
    fn report(&self, _pct: Option<i64>, _msg: Option<String>) {}
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
    /// See [`StateBackend`]. Defaults to [`NoState`] everywhere but
    /// `handler.rs`'s real dispatch path, which wires this call's own
    /// tenant into `tenant_state.rs`.
    pub state: Arc<dyn StateBackend>,
    /// PRD-mcphost-composition requirement 2: how many levels of
    /// composition already led to this call -- `0` for every ordinary
    /// top-level `tools/call`/`host.tool_call`. [`compose_call`] refuses a
    /// dispatch that would push this past [`COMPOSE_DEPTH_MAX`].
    pub compose_depth: u32,
    /// PRD-mcphost-composition requirement 2: the running count of child
    /// calls made anywhere in this call's whole tree, shared (via the
    /// `Arc`) by every node so the ceiling ([`COMPOSE_CHILDREN_MAX`]) is
    /// per-tree, not per-node. `None` means this call is not part of a
    /// composable tree (composition unavailable) -- [`compose_call`]
    /// refuses rather than silently skipping the ceiling.
    pub compose_children: Option<Arc<std::sync::atomic::AtomicU32>>,
    /// PRD-mcphost-composition requirement 1: the tenant-tool table
    /// [`compose_call`] resolves a composed call's target name against.
    /// `None` in any context composition isn't wired into (most of
    /// `host.tool_test`/`host.tool_run` today) -- a `Kind` that tries to
    /// compose there gets a clear "unavailable" error rather than a panic.
    pub compose_db: Option<crate::db::Db>,
    /// PRD-mcphost-composition requirement 1: the kind registry
    /// [`compose_call`] dispatches a composed call's target through. See
    /// [`CallCtx::compose_db`]'s note on when this is `None`.
    pub compose_kinds: Option<KindRegistry>,
    /// PRD-mcphost-call-limits-honest requirement 3: this call's tenant's
    /// per-tenant concurrent-call admission cap, resolved by `handler.rs`
    /// from the tenant's plan (`PlanCatalog`) before dispatch -- a `Kind`
    /// with per-tenant admission control (`python`) reads this rather than
    /// reaching into `AppState` itself, matching every other resolved-value
    /// field this struct already carries (`deadline`, `secrets`).
    /// `usize::MAX` (effectively unbounded) in every context with no notion
    /// of a tenant plan (`for_test`, the conformance suite, `python.rs`'s
    /// own unit tests) -- unchanged behavior for every test that doesn't
    /// exercise per-tenant admission control.
    pub concurrent_calls_per_tenant: usize,
    /// PRD-mcphost-runs-and-jobs requirement 4: this call's own `runs.id`,
    /// set only when this dispatch IS a job execution (the executor's own
    /// call into `Kind::call`, `runs.rs::run_one_job`). `None` for every
    /// ordinary synchronous call (`call_published_tool`'s own path,
    /// `for_test`, the conformance suite) -- a `Kind` never needs to branch
    /// on this itself; it exists so `ctx.progress`/`ctx.cancel_pid` have
    /// somewhere meaningful to report to.
    pub run_id: Option<String>,
    /// See [`ProgressSink`]. Defaults to [`NullProgress`] everywhere but
    /// the executor's job-dispatch path.
    pub progress: Arc<dyn ProgressSink>,
    /// PRD-mcphost-runs-and-jobs requirement 4 / AC4 (`host.runs.cancel`):
    /// the pid of the sandboxed subprocess currently serving this call, if
    /// any -- set by `kinds::python`'s call path around each
    /// `PersistentSandbox::call` round trip (warm or cold), cleared the
    /// instant that round trip returns. `host.runs.cancel` (via
    /// `AppState.runs`'s registry, which holds the SAME `Arc` this field
    /// holds for a job's `CallCtx`) reads this to `killpg` the actual OS
    /// process group -- independent of whatever the `Kind::call` future
    /// itself is doing, so cancellation works even though `Kind::call` is a
    /// plain `async fn` with no cooperative-cancellation contract of its
    /// own. A kind with no subprocess (`echo`, `http`) never touches this;
    /// it just stays `None` for the whole call, and `host.runs.cancel`
    /// degrades to "mark cancelled in the ledger, nothing to kill" (still
    /// correct: an `http` call has no process to leave running either).
    pub cancel_pid: Arc<Mutex<Option<i32>>>,
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
            state: Arc::new(NoState),
            compose_depth: 0,
            compose_children: None,
            compose_db: None,
            compose_kinds: None,
            concurrent_calls_per_tenant: usize::MAX,
            run_id: None,
            progress: Arc::new(NullProgress),
            cancel_pid: Arc::new(Mutex::new(None)),
        }
    }

    pub fn time_remaining(&self) -> Duration {
        self.deadline.saturating_duration_since(Instant::now())
    }
}

/// PRD-mcphost-composition requirement 2: how many levels of composed
/// dispatch [`compose_call`] allows before refusing with
/// `compose_depth_exceeded` -- a call already at this depth may not
/// dispatch one more (AC4: nested five deep, the fifth is refused, naming
/// this limit).
pub const COMPOSE_DEPTH_MAX: u32 = 4;

/// PRD-mcphost-composition requirement 2: how many child calls a single
/// top-level call tree may make in total (shared across every node via
/// [`CallCtx::compose_children`]) before [`compose_call`] refuses with
/// `compose_children_exceeded`.
pub const COMPOSE_CHILDREN_MAX: u32 = 50;

/// PRD-mcphost-composition requirements 1/2: dispatches `target_name` (in
/// `tenant_id`'s tool table) as a child of whatever call `ctx` belongs to.
/// The one entry point every composing `Kind` uses -- today `chain`'s step
/// dispatch; once the sandbox grows a nested-call channel, `python`'s
/// `mcphost.call` calls this too, unchanged.
///
/// Enforces, in order: requirement 2's immediate self-call refusal
/// (`compose_self_call`), the depth ceiling (`compose_depth_exceeded`,
/// [`COMPOSE_DEPTH_MAX`]), and the per-tree children ceiling
/// (`compose_children_exceeded`, [`COMPOSE_CHILDREN_MAX`]) -- all before
/// `target_name` is even looked up, so a refusal never touches the tool
/// table. `ctx.compose_children`/`compose_db`/`compose_kinds` all being
/// `Some` is this function's precondition for anything past the ceiling
/// checks; a caller with any of them `None` gets a plain `Exec` error
/// rather than a panic (composition not wired into this call context --
/// `host.tool_test`/`host.tool_run` today).
///
/// PRD-mcphost-runs-and-jobs (this PRD's declared dependency) has not
/// shipped: there is no `runs` table, so this records no child run row --
/// it dispatches and returns the child's result, and a caller that wants a
/// receipt (`chain`'s own `steps` trace) builds it from this call's
/// `Ok`/`Err` itself. Wiring a real child run here is the follow-up once
/// that PRD lands.
pub async fn compose_call(
    ctx: &CallCtx,
    tenant_id: i64,
    target_name: &str,
    args: Value,
    timeout_s: Option<u64>,
) -> Result<Value, KindError> {
    if ctx.tool_name.as_deref() == Some(target_name) {
        return Err(KindError::structured(
            "compose_self_call",
            format!("tool '{target_name}' cannot call itself"),
        ));
    }

    let next_depth = ctx.compose_depth + 1;
    if next_depth > COMPOSE_DEPTH_MAX {
        return Err(KindError::structured_with(
            "compose_depth_exceeded",
            format!("composition nesting exceeds the limit of {COMPOSE_DEPTH_MAX}"),
            json!({"limit": COMPOSE_DEPTH_MAX}),
        ));
    }

    let Some(children) = ctx.compose_children.as_ref() else {
        return Err(KindError::Exec(
            "composition is unavailable in this call context".into(),
        ));
    };
    let used = children.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
    if used > COMPOSE_CHILDREN_MAX {
        return Err(KindError::structured_with(
            "compose_children_exceeded",
            format!("this call tree has exceeded {COMPOSE_CHILDREN_MAX} child calls"),
            json!({"limit": COMPOSE_CHILDREN_MAX}),
        ));
    }

    let Some(db) = ctx.compose_db.as_ref() else {
        return Err(KindError::Exec(
            "composition is unavailable in this call context".into(),
        ));
    };
    let Some(kinds) = ctx.compose_kinds.as_ref() else {
        return Err(KindError::Exec(
            "composition is unavailable in this call context".into(),
        ));
    };

    let row = db
        .get_tool(tenant_id, target_name.to_string())
        .await
        .map_err(|e| KindError::Exec(format!("tool lookup failed: {e}")))?
        .ok_or_else(|| {
            KindError::structured("tool_not_found", format!("no such tool: {target_name}"))
        })?;
    let kind = kinds.get(&row.kind).ok_or_else(|| {
        KindError::Exec(format!(
            "published tool names unregistered kind '{}'",
            row.kind
        ))
    })?;

    let descriptor = kind.describe(&row.spec);
    if let Ok(validator) = jsonschema::validator_for(&descriptor.input_schema)
        && let Err(e) = validator.validate(&args)
    {
        let data = describe_args_error(&e);
        return Err(KindError::Structured {
            code: "args_invalid",
            message: e.to_string(),
            data,
        });
    }

    // Requirement 1: an explicit `timeout_s` further bounds this one child
    // call, but never past the parent's own remaining deadline -- "a
    // parent's deadline bounds the whole tree" (Technical considerations),
    // so a child can only ask for less time, never more.
    let deadline = match timeout_s {
        Some(secs) => std::cmp::min(ctx.deadline, Instant::now() + Duration::from_secs(secs)),
        None => ctx.deadline,
    };

    let child_ctx = CallCtx {
        tenant_id,
        namespace: ctx.namespace.clone(),
        secrets: ctx.secrets.clone(),
        deadline,
        log: ctx.log.clone(),
        test_mode: false,
        resources: ctx.resources.clone(),
        tool_name: Some(target_name.to_string()),
        // PRD-mcphost-tenant-state: composition stays inside one tenant
        // (Non-goals), so the child's `mcphost.state` reaches the exact
        // same tenant's store the parent's does -- no re-derivation needed.
        state: ctx.state.clone(),
        compose_depth: next_depth,
        compose_children: Some(children.clone()),
        compose_db: Some(db.clone()),
        compose_kinds: Some(kinds.clone()),
        // PRD-mcphost-call-limits-honest requirement 3: a composed child
        // call is still the same tenant's traffic against the same
        // per-tenant admission cap -- inherit the parent's resolved value
        // rather than re-deriving it (this call site has no `Tenant`/plan
        // to resolve from, only the already-dispatched parent `ctx`).
        concurrent_calls_per_tenant: ctx.concurrent_calls_per_tenant,
        // PRD-mcphost-runs-and-jobs: a composed child call is still part of
        // the same job (if any) the parent is executing -- progress and
        // cancellation both reach through to the same run, so a chained
        // tool's own `mcphost.progress` calls land on the parent job's
        // `runs` row too, and `host.runs.cancel` still kills whichever
        // sandbox (parent or child) is currently in flight.
        run_id: ctx.run_id.clone(),
        progress: ctx.progress.clone(),
        cancel_pid: ctx.cancel_pid.clone(),
    };

    kind.call(&row.spec, args, &child_ctx).await
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

    /// PRD-mcphost-call-limits-honest requirement 1: the per-call deadline
    /// this `spec` itself declares (already bounded by the kind's own
    /// maximum, e.g. `python`'s `MAX_TIMEOUT_S`), if any. Every real
    /// dispatch site in `handler.rs` uses this in place of
    /// `AppState::call_timeout` when it returns `Some` -- `CALL_TIMEOUT`
    /// (or its `AppState` field) is the *default* a spec falls back to when
    /// this returns `None`, not a ceiling every call is silently held to
    /// regardless of what it declared. Defaults to `None` (unchanged
    /// behavior) for every kind with no per-tool notion of a timeout
    /// (`echo`, `http`); `python` is this PRD's only override.
    fn requested_timeout(&self, _spec: &Value) -> Option<Duration> {
        None
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

    /// PRD-mcphost-first-call-reliability requirement 4: a tool of this kind
    /// was just published (or republished) successfully -- pre-provision
    /// whatever out-of-process state (e.g. a build) the common "publish,
    /// then call a few seconds later" case would otherwise pay for at call
    /// time. Called with the freshly-stored `spec` right after
    /// `host.tool_publish` writes it, never awaited by the caller (a slow
    /// or failed provision here must not delay or fail the publish RPC).
    /// Kinds with no such notion (`echo`, `http`) don't override this.
    async fn on_tool_published(
        &self,
        _tenant_id: i64,
        _namespace: &str,
        _local_name: &str,
        _spec: &Value,
    ) {
    }

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

    /// PRD-mcphost-result-envelope-contract requirement 1 (extended by
    /// PRD-mcphost-spec-output-paths requirement 2/3 to carry each entry's
    /// optional [`Path`]): the output fields this `spec` declares (its own
    /// `outputs`, when present) -- `Kind::call` promotes each to
    /// `result.payload.<field>` (directly from its `path`, when it has one,
    /// else via the wrapper search) and `host.tool_test` reports any that
    /// never land there. `Vec::new()` (the default) for a kind with no
    /// declared-outputs concept, or an unparseable spec (publish-time
    /// validation already rejects that spec before this is ever reached in
    /// practice; this fallback exists only so the method never panics) -- a
    /// tool that declares nothing gets no envelope changes at all (additive,
    /// see Migration/compatibility).
    fn declared_outputs(&self, _spec: &Value) -> Vec<OutputDecl> {
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

    /// PRD-mcphost-spec-output-paths requirement 5: the *un-promoted* body
    /// [`envelope_report`]'s `seen_at` hint searches for a bare-name
    /// declared field the contract path never got -- distinct from
    /// [`Kind::payload_from_call_result`] (which is already the promoted
    /// result and so, by definition, never contains the missing field at any
    /// depth `find_seen_at` would need to search). Defaults to the same spot
    /// `payload_from_call_result` reads, which is a reasonable
    /// (non-panicking) answer for a kind that doesn't override either; the
    /// `http` kind overrides this to point at the upstream response body
    /// instead, since that -- not the already-promoted payload -- is what
    /// AC6's `seen_at` example is found in.
    fn source_for_output_search<'a>(&self, call_result: &'a Value) -> Option<&'a Value> {
        self.payload_from_call_result(call_result)
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

#[cfg(test)]
mod spec_output_paths_tests {
    use super::*;

    #[test]
    fn path_parses_dotted_keys_and_bracketed_indices() {
        let path = Path::parse("$.a.b[0].c").expect("valid path");
        let source = json!({"a": {"b": [{"c": "found"}]}});
        assert_eq!(path.resolve(&source), Some(&json!("found")));
        assert_eq!(path.as_str(), "$.a.b[0].c");
    }

    #[test]
    fn path_resolve_returns_none_rather_than_erroring_on_a_miss() {
        let path = Path::parse("$.a.b").expect("valid path");
        assert_eq!(path.resolve(&json!({"a": {}})), None);
        assert_eq!(path.resolve(&json!({"x": 1})), None);
        assert_eq!(path.resolve(&json!([1, 2, 3])), None);
    }

    #[test]
    fn path_rejects_a_wildcard_filter_and_recursive_descent() {
        assert_eq!(Path::parse("$.items[*].a"), Err(PathParseError::Wildcard));
        assert_eq!(Path::parse("$.*"), Err(PathParseError::Wildcard));
        assert_eq!(Path::parse("$.items[?(@.ok)]"), Err(PathParseError::Filter));
        assert_eq!(Path::parse("$..a"), Err(PathParseError::RecursiveDescent));
        assert_eq!(Path::parse("a.b"), Err(PathParseError::Malformed));
        assert_eq!(Path::parse("$.a[x]"), Err(PathParseError::Malformed));
    }

    #[test]
    fn normalize_outputs_accepts_the_list_form_unchanged() {
        let decls = normalize_outputs(&json!(["status", "score"])).expect("valid list");
        assert_eq!(
            decls,
            vec![
                OutputDecl {
                    name: "status".to_string(),
                    path: None
                },
                OutputDecl {
                    name: "score".to_string(),
                    path: None
                },
            ]
        );
    }

    #[test]
    fn normalize_outputs_accepts_the_map_form() {
        let decls = normalize_outputs(&json!({"a": "$.json.a"})).expect("valid map");
        assert_eq!(decls.len(), 1);
        assert_eq!(decls[0].name, "a");
        assert_eq!(decls[0].path.as_ref().map(Path::as_str), Some("$.json.a"));
    }

    #[test]
    fn normalize_outputs_rejects_a_non_string_map_value() {
        let err = normalize_outputs(&json!({"a": 5})).unwrap_err();
        let KindError::Structured { code, message, data } = err else {
            panic!("expected a Structured error");
        };
        assert_eq!(code, "invalid_spec");
        assert_eq!(data["field"], "outputs.a");
        assert_eq!(data["got"], "number");
        assert!(message.starts_with("invalid spec: outputs.a:"));
    }

    #[test]
    fn normalize_outputs_rejects_an_unsupported_top_level_shape() {
        let err = normalize_outputs(&json!("not a list or map")).unwrap_err();
        let KindError::Structured { data, .. } = err else {
            panic!("expected a Structured error");
        };
        assert_eq!(data["field"], "outputs");
        assert_eq!(data["got"], "string");
    }

    #[test]
    fn find_seen_at_finds_a_key_nested_up_to_depth_three() {
        let source = json!({"json": {"ingestion_status": "ok"}});
        let hints = find_seen_at(&source, "ingestion_status", 3, 3);
        assert_eq!(hints, vec!["$.json.ingestion_status".to_string()]);
    }

    #[test]
    fn find_seen_at_respects_the_depth_and_result_bounds() {
        let deep = json!({"a": {"b": {"c": {"target": 1}}}});
        // depth 3 only reaches a.b.c, not a.b.c.target (depth 4).
        assert!(find_seen_at(&deep, "target", 3, 3).is_empty());
        assert_eq!(find_seen_at(&deep, "target", 4, 3), vec!["$.a.b.c.target"]);

        let many = json!({"a": 1, "b": 1, "c": 1, "d": 1});
        assert_eq!(find_seen_at(&many, "z", 3, 0).len(), 0);
    }

    #[test]
    fn envelope_report_keeps_missing_as_plain_names_and_adds_detail() {
        let declared = vec![
            OutputDecl {
                name: "found_field".to_string(),
                path: None,
            },
            OutputDecl {
                name: "absent_with_path".to_string(),
                path: Some(Path::parse("$.json.absent").unwrap()),
            },
            OutputDecl {
                name: "absent_bare".to_string(),
                path: None,
            },
        ];
        let payload = json!({"found_field": "x"});
        let seen_at_source = json!({"json": {"absent_bare": "here"}});
        let report =
            envelope_report(&declared, Some(&payload), Some(&seen_at_source)).expect("some report");

        let missing = report["missing"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>();
        assert_eq!(missing, vec!["absent_with_path", "absent_bare"]);

        let detail = report["missing_detail"].as_array().unwrap();
        let with_path = detail
            .iter()
            .find(|d| d["name"] == "absent_with_path")
            .unwrap();
        assert_eq!(with_path["path"], "$.json.absent");

        let bare = detail.iter().find(|d| d["name"] == "absent_bare").unwrap();
        assert_eq!(bare["seen_at"], json!(["$.json.absent_bare"]));

        assert_eq!(report["found_at_contract_path"], json!(["found_field"]));
        assert_eq!(report["green"], false);
    }

    /// PRD-mcphost-spec-output-paths AC1/AC4, reviewer-agent counter_attack
    /// at 708cf46 ("declared-path-silently-shadowed-by-native-top-level-
    /// key"): `payload` starts as a clone of the raw body (http.rs seeds it
    /// that way for the "payload mirrors body" shape), so a body whose top
    /// level already has a same-named key must not let that native value
    /// win over a declared path -- the path is what the publisher wrote and
    /// is what must land at `result.payload.<name>`.
    #[test]
    fn apply_output_decls_lets_a_declared_path_win_over_a_colliding_native_top_level_key() {
        let declared = vec![OutputDecl {
            name: "bridge_status".to_string(),
            path: Some(Path::parse("$.json.bridge_status").unwrap()),
        }];
        let source = json!({"bridge_status": "stale", "json": {"bridge_status": "active"}});
        // http.rs seeds payload as a full clone of the body before calling
        // apply_output_decls -- reproduce that here.
        let mut payload = source.as_object().unwrap().clone();

        apply_output_decls(&mut payload, &source, &declared, Some(&ENVELOPE_WRAPPER_KEYS));

        assert_eq!(payload["bridge_status"], "active");
    }

    /// Same shadowing bug, the AC5 half: a path that fails to resolve must
    /// leave the field genuinely absent, even when the raw body already had
    /// a same-named top-level key -- "absent" means the path's own
    /// resolution, not whatever the body happened to put there.
    #[test]
    fn apply_output_decls_removes_a_colliding_native_key_when_the_declared_path_misses() {
        let declared = vec![OutputDecl {
            name: "bridge_status".to_string(),
            path: Some(Path::parse("$.json.absent").unwrap()),
        }];
        let source = json!({"bridge_status": "stale", "json": {}});
        let mut payload = source.as_object().unwrap().clone();

        apply_output_decls(&mut payload, &source, &declared, Some(&ENVELOPE_WRAPPER_KEYS));

        assert!(!payload.contains_key("bridge_status"));
    }
}
