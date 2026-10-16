//! The `jq` kind (PRD-mcphost-jq-kind): `spec.program` is a jq filter run
//! in-process under `jaq` with `.` bound to the call's arguments and
//! `$prev` bound to the previous chain step's result (`null` outside a
//! chain). No subprocess, no sandbox: the program is pure computation over
//! its input, and the budgets below are enforced by the evaluator thread
//! itself.
//!
//! ## Errors
//!
//! * A program that does not parse is refused at publish as `invalid_spec`
//!   carrying `data.line`, `data.column` and `data.source_line` -- the
//!   position of the token the parser stopped at.
//! * A program that names a builtin in [`EXCLUDED_BUILTINS`] (`input`,
//!   `env`, `@sh`, ...) is refused at publish as `jq_builtin_unsupported`,
//!   listing [`allowed_builtins`].
//! * A runtime error (`jaq`'s `Error`) is `tool_exception` with
//!   `data.exception_class: "jq_error"`; `jaq` errors carry no span, so
//!   `source_line` is the program's first line and `position_known` false.
//!
//! ## Budgets
//!
//! The evaluator checks its deadline between output values, so a program
//! that keeps yielding (`repeat(.)`) ends in `tool_timeout`. A single term
//! that never yields (`[repeat(.)]`) cannot be interrupted -- `jaq` has no
//! step limit -- so the call still answers `tool_timeout` on time while the
//! evaluator thread is abandoned (told to stop at its next output) and
//! counted by [`abandoned_evaluators`].
//!
//! Output over `max_output_bytes` stops being stored but the program keeps
//! running until it ends or the deadline passes: finite oversized output is
//! `tool_output_too_large`, a runaway program is `tool_timeout`.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use jaq_core::load::{Arena, File, Loader};
use jaq_core::{Compiler, Ctx, Vars, data, unwrap_valr};
use jaq_json::Val;
use serde::Deserialize as _;
use serde_json::{Value, json};

use super::contract::{
    BudgetDim, ConformanceFixtures, ErrorFamily, KindContract, SourcePosition, codes, family, key,
};
use super::{CallCtx, Kind, KindError, KindExample, OutputDecl, ToolDescriptor};

const MIN_TIMEOUT_S: u64 = 1;
const MAX_TIMEOUT_S: u64 = 10;
const DEFAULT_TIMEOUT_S: u64 = 2;
const MIN_OUTPUT_BYTES: u64 = 1024;
const MAX_OUTPUT_BYTES: u64 = 1024 * 1024;
const DEFAULT_OUTPUT_BYTES: u64 = 256 * 1024;
/// Added to `timeout_s` for the host's outer per-call deadline and for the
/// async guard around the evaluator thread, so the evaluator's own
/// `tool_timeout` (raised at `timeout_s`) wins the race.
const GUARD_SLACK: Duration = Duration::from_millis(500);
const OUTER_SLACK: Duration = Duration::from_millis(1000);
const EVALUATOR_STACK_BYTES: usize = 16 * 1024 * 1024;
const PREV_VAR: &str = "$prev";
const EXCLUSION_REASON: &str = "streaming input, file/process/environment access and modules are not part of the jq kind";

/// Builtins the jq kind refuses at publish (Non-goals): streaming input,
/// `@sh`, process/environment access and halting. `import`/`include` are
/// refused by the loader and reported under the name `import`.
pub const EXCLUDED_BUILTINS: &[&str] = &[
    "input",
    "inputs",
    "input_filename",
    "input_line_number",
    "env",
    "$ENV",
    "$__loc__",
    "@sh",
    "escape_sh",
    "halt",
    "halt_error",
    "stderr",
    "stderr_empty",
    "import",
    "include",
];

type D = data::JustLut<Val>;
type JqFilter = jaq_core::Filter<D>;
type Fun = jaq_core::native::Fun<D>;
type Def = jaq_core::load::parse::Def<&'static str>;

fn all_defs() -> Vec<Def> {
    jaq_core::defs().chain(jaq_std::defs()).chain(jaq_json::defs()).collect()
}

fn all_funs() -> Vec<Fun> {
    jaq_core::funs::<D>().chain(jaq_std::funs::<D>()).chain(jaq_json::funs::<D>()).collect()
}

fn is_excluded(name: &str) -> bool {
    EXCLUDED_BUILTINS.contains(&name)
}

fn render(name: &str, arity: usize) -> String {
    format!("{name}/{arity}")
}

/// Every `name/arity` the `jaq` libraries define, exclusions included.
pub fn registered_builtins() -> Vec<String> {
    let mut all: Vec<String> = all_defs()
        .iter()
        .map(|d| render(d.name, d.args.len()))
        .chain(all_funs().iter().map(|(name, args, _)| render(name, args.len())))
        .collect();
    all.sort();
    all.dedup();
    all
}

/// The `name/arity` set programs may use: what [`compile`] hands the loader
/// and compiler, i.e. the registered definitions minus [`EXCLUDED_BUILTINS`].
pub fn allowed_builtins() -> Vec<String> {
    let mut allowed: Vec<String> = allowed_defs()
        .iter()
        .map(|d| render(d.name, d.args.len()))
        .chain(allowed_funs().iter().map(|(name, args, _)| render(name, args.len())))
        .collect();
    allowed.sort();
    allowed.dedup();
    allowed
}

fn allowed_defs() -> Vec<Def> {
    all_defs().into_iter().filter(|d| !is_excluded(d.name)).collect()
}

fn allowed_funs() -> Vec<Fun> {
    all_funs().into_iter().filter(|(name, _, _)| !is_excluded(name)).collect()
}

/// How many evaluator threads were abandoned because a single term never
/// yielded to the deadline check.
pub fn abandoned_evaluators() -> u64 {
    ABANDONED.load(Ordering::Relaxed)
}

static ABANDONED: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone)]
struct JqSpec {
    program: String,
    args_schema: Option<Value>,
    outputs: Vec<OutputDecl>,
    timeout_s: u64,
    max_output_bytes: u64,
    description: Option<String>,
}

impl JqSpec {
    fn effective_args_schema(&self) -> Value {
        self.args_schema.clone().unwrap_or_else(|| json!({"type": "object"}))
    }
}

fn bounded(spec: &Value, field: &str, min: u64, max: u64, default: u64) -> Result<u64, KindError> {
    match spec.get(field) {
        None | Some(Value::Null) => Ok(default),
        Some(v) => {
            let n = v
                .as_u64()
                .ok_or_else(|| KindError::InvalidSpec(format!("{field}: must be a positive integer")))?;
            if (min..=max).contains(&n) {
                Ok(n)
            } else {
                Err(KindError::InvalidSpec(format!("{field}: must be between {min} and {max}; got {n}")))
            }
        }
    }
}

fn parse_spec(spec: &Value) -> Result<JqSpec, KindError> {
    if !spec.is_object() {
        return Err(KindError::InvalidSpec("spec: must be a JSON object".into()));
    }
    let program = spec
        .get("program")
        .and_then(Value::as_str)
        .ok_or_else(|| KindError::InvalidSpec("program: is required (a jq program string)".into()))?
        .to_string();
    let args_schema = match spec.get("args_schema") {
        None | Some(Value::Null) => None,
        Some(schema) => {
            jsonschema::validator_for(schema)
                .map_err(|e| KindError::InvalidSpec(format!("args_schema: not a valid JSON Schema: {e}")))?;
            Some(schema.clone())
        }
    };
    let outputs = match spec.get("outputs") {
        None => Vec::new(),
        Some(v) => super::normalize_outputs(v)?,
    };
    let description = match spec.get("description") {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) => Some(s.clone()),
        Some(_) => return Err(KindError::InvalidSpec("description: must be a string".into())),
    };
    Ok(JqSpec {
        program,
        args_schema,
        outputs,
        timeout_s: bounded(spec, "timeout_s", MIN_TIMEOUT_S, MAX_TIMEOUT_S, DEFAULT_TIMEOUT_S)?,
        max_output_bytes: bounded(spec, "max_output_bytes", MIN_OUTPUT_BYTES, MAX_OUTPUT_BYTES, DEFAULT_OUTPUT_BYTES)?,
        description,
    })
}

/// 1-based `(line, column, line text)` of byte `offset` in `code`; `column`
/// counts characters.
fn position_of(code: &str, offset: usize) -> (usize, usize, String) {
    let mut offset = offset.min(code.len());
    while !code.is_char_boundary(offset) {
        offset -= 1;
    }
    let before = &code[..offset];
    let line = before.matches('\n').count() + 1;
    let line_start = before.rfind('\n').map_or(0, |i| i + 1);
    let column = before[line_start..].chars().count() + 1;
    let line_end = code[offset..].find('\n').map_or(code.len(), |i| offset + i);
    (line, column, code[line_start..line_end].to_string())
}

fn offset_in(code: &str, part: &str) -> usize {
    let (base, at) = (code.as_ptr() as usize, part.as_ptr() as usize);
    if at >= base && at <= base + code.len() { at - base } else { 0 }
}

fn first_line(code: &str) -> &str {
    code.lines().next().unwrap_or("")
}

fn unsupported(name: &str) -> KindError {
    let allowed = allowed_builtins();
    KindError::structured_with(
        codes::JQ_BUILTIN_UNSUPPORTED,
        format!("program: `{name}` is not supported by the jq kind ({EXCLUSION_REASON}); allowed builtins: {}", allowed.join(", ")),
        json!({
            "field": "program",
            "builtin": name,
            "excluded": EXCLUDED_BUILTINS,
            "allowed": allowed,
        }),
    )
}

fn syntax_error(code: &str, found: &str, expected: &str) -> KindError {
    let (line, column, source_line) = position_of(code, offset_in(code, found));
    KindError::structured_with(
        "invalid_spec",
        format!("program: syntax error at line {line}, column {column}: expected {expected}"),
        json!({
            "field": "program",
            "exception_class": "JqSyntaxError",
            "line": line,
            "column": column,
            "source_line": source_line,
        }),
    )
}

/// Parses and compiles `program`. The only place the loader and compiler
/// are built, so [`allowed_builtins`] is exactly what a program can call.
fn compile(program: &str) -> Result<JqFilter, KindError> {
    let loader = Loader::new(allowed_defs());
    let arena = Arena::default();
    let modules = loader.load(&arena, File { code: program, path: () }).map_err(|errs| {
        let mut first = None;
        for (_, err) in errs {
            first = first.or(Some(match err {
                jaq_core::load::Error::Io(_) => unsupported("import"),
                jaq_core::load::Error::Lex(errs) => match errs.into_iter().next() {
                    Some((expect, found)) => syntax_error(program, found, expect.as_str()),
                    None => KindError::InvalidSpec("program: could not be lexed".into()),
                },
                jaq_core::load::Error::Parse(errs) => match errs.into_iter().next() {
                    Some((expect, found)) => syntax_error(program, found, expect.as_str()),
                    None => KindError::InvalidSpec("program: could not be parsed".into()),
                },
            }));
        }
        first.unwrap_or_else(|| KindError::InvalidSpec("program: could not be loaded".into()))
    })?;
    Compiler::default()
        .with_funs(allowed_funs())
        .with_global_vars([PREV_VAR])
        .compile(modules)
        .map_err(|errs| {
            let mut first = None;
            for (_, undefined) in errs {
                for (name, what) in undefined {
                    first = first.or(Some(if is_excluded(name) {
                        unsupported(name)
                    } else {
                        let (line, column, source_line) = position_of(program, offset_in(program, name));
                        KindError::structured_with(
                            "invalid_spec",
                            format!("program: undefined {} `{name}` at line {line}, column {column}", what.as_str()),
                            json!({
                                "field": "program",
                                "exception_class": "JqCompileError",
                                "line": line,
                                "column": column,
                                "source_line": source_line,
                            }),
                        )
                    }));
                }
            }
            first.unwrap_or_else(|| KindError::InvalidSpec("program: could not be compiled".into()))
        })
}

/// Compiled filters by program text, so a hot tool pays the parse once.
fn compiled(program: &str) -> Result<Arc<JqFilter>, KindError> {
    static CACHE: OnceLock<Mutex<HashMap<String, Arc<JqFilter>>>> = OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    if let Some(hit) = cache.lock().unwrap().get(program) {
        return Ok(hit.clone());
    }
    let filter = Arc::new(compile(program)?);
    let mut guard = cache.lock().unwrap();
    if guard.len() >= 256 {
        guard.clear();
    }
    guard.insert(program.to_string(), filter.clone());
    Ok(filter)
}

fn to_val(v: &Value) -> Val {
    Val::deserialize(v).unwrap_or(Val::Null)
}

fn from_val(v: &Val) -> Value {
    let mut buf = Vec::new();
    match jaq_json::write::write(&mut buf, &jaq_json::write::Pp::default(), 0, v) {
        Ok(()) => serde_json::from_slice(&buf).unwrap_or(Value::Null),
        Err(_) => Value::Null,
    }
}

enum Finished {
    Outputs(Vec<Value>),
    TooLarge { bytes: u64 },
    Timeout,
    Failed(String),
}

struct Limits {
    deadline: Instant,
    max_bytes: u64,
}

fn run_filter(filter: &JqFilter, input: Value, prev: Value, limits: &Limits, cancel: &AtomicBool) -> Finished {
    let Limits { deadline, max_bytes } = *limits;
    let ctx = Ctx::<D>::new(&filter.lut, Vars::new([to_val(&prev)]));
    let mut outputs = Vec::new();
    let mut bytes: u64 = 0;
    for result in filter.id.run((ctx, to_val(&input))).map(unwrap_valr) {
        if cancel.load(Ordering::Relaxed) || Instant::now() >= deadline {
            return Finished::Timeout;
        }
        match result {
            Err(e) => return Finished::Failed(e.to_string()),
            Ok(val) => {
                let out = from_val(&val);
                bytes += out.to_string().len() as u64;
                // Over the cap: stop storing, keep draining until the
                // program ends (too large) or the deadline passes (timeout).
                if bytes <= max_bytes {
                    outputs.push(out);
                }
            }
        }
    }
    if bytes > max_bytes { Finished::TooLarge { bytes } } else { Finished::Outputs(outputs) }
}

fn type_name(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

/// `jaq` words an indexing error with the offending values
/// (`cannot index 1 with "b"`); the jq an agent knows names their types
/// (`Cannot index number with "b"`). Any other message is only capitalized.
fn jq_style(message: &str) -> String {
    if let Some(rest) = message.strip_prefix("cannot index ") {
        for (at, _) in rest.match_indices(" with ") {
            let (Ok(left), Ok(right)) = (
                serde_json::from_str::<Value>(&rest[..at]),
                serde_json::from_str::<Value>(&rest[at + " with ".len()..]),
            ) else {
                continue;
            };
            let key = if right.is_string() { right.to_string() } else { type_name(&right).to_string() };
            return format!("Cannot index {} with {key}", type_name(&left));
        }
    }
    let mut chars = message.chars();
    chars.next().map_or_else(String::new, |c| c.to_uppercase().chain(chars).collect())
}

fn conformance_good_spec() -> Value {
    json!({
        "program": ".msg",
        "args_schema": {"type": "object", "properties": {"msg": {"type": "string"}}, "required": ["msg"]},
    })
}

fn conformance_erroring_spec() -> Value {
    json!({"program": "error(\"boom\")"})
}

fn conformance_slow_spec() -> Value {
    json!({"program": "repeat(.)"})
}

const JQ_TOOL_CODE: ErrorFamily = ErrorFamily { name: family::TOOL_CODE, codes: &[codes::TOOL_EXCEPTION] };
const JQ_TOOL_LIMIT: ErrorFamily = ErrorFamily {
    name: family::TOOL_LIMIT,
    codes: &[codes::TOOL_TIMEOUT, codes::TOOL_OUTPUT_TOO_LARGE],
};

static CONTRACT: KindContract = KindContract {
    error_families: &[JQ_TOOL_CODE, JQ_TOOL_LIMIT],
    budgets: &[
        BudgetDim { field: "timeout_s", min: MIN_TIMEOUT_S, max: MAX_TIMEOUT_S, default: DEFAULT_TIMEOUT_S },
        BudgetDim { field: "max_output_bytes", min: MIN_OUTPUT_BYTES, max: MAX_OUTPUT_BYTES, default: DEFAULT_OUTPUT_BYTES },
    ],
    source_position: SourcePosition::LineColumn,
    bridges: &[],
    secret_redaction: false,
    fixtures: ConformanceFixtures {
        good_spec: conformance_good_spec,
        good_args: || json!({"msg": "hi"}),
        bad_args: || json!({"msg": 5}),
        erroring_spec: Some(conformance_erroring_spec),
        slow_spec: Some(conformance_slow_spec),
        secret_spec: None,
        skips: &[],
    },
};

#[derive(Default)]
pub struct JqKind;

#[async_trait::async_trait]
impl Kind for JqKind {
    fn contract(&self) -> &'static KindContract {
        &CONTRACT
    }

    fn name(&self) -> &'static str {
        "jq"
    }

    fn validate(&self, spec: &Value) -> Result<(), KindError> {
        let parsed = parse_spec(spec)?;
        compile(&parsed.program).map(|_| ())
    }

    fn known_spec_fields(&self) -> &'static [&'static str] {
        &["program", "args_schema", "timeout_s", "max_output_bytes", "description", "outputs"]
    }

    fn required_spec_field(&self) -> Option<&'static str> {
        Some("program")
    }

    fn field_docs(&self) -> &'static [super::kinddoc::FieldDoc] {
        use super::kinddoc::FieldDoc;
        &[
            FieldDoc { name: "program", ty: "string", meaning: "the jq program, run over the call arguments (`.`); `$prev` holds the previous chain step's result" },
            FieldDoc { name: "args_schema", ty: "object", meaning: "JSON Schema for the call arguments (defaults to `{\"type\": \"object\"}`)" },
            FieldDoc { name: "timeout_s", ty: "integer", meaning: "seconds the program may run (1-10, default 2)" },
            FieldDoc { name: "max_output_bytes", ty: "integer", meaning: "bytes of output a call may return (1 KiB-1 MiB, default 256 KiB)" },
            FieldDoc { name: "description", ty: "string", meaning: "text shown to callers in the tool listing" },
            FieldDoc { name: "outputs", ty: "array or object", meaning: "fields a caller can rely on in `result.payload`" },
        ]
    }

    fn ceiling_fields(&self) -> &'static [&'static str] {
        &["budget.max_tool_latency_ms", "budget.max_wall_ms"]
    }

    fn describe(&self, spec: &Value) -> ToolDescriptor {
        match parse_spec(spec) {
            Ok(parsed) => ToolDescriptor {
                name: "jq".to_string(),
                description: parsed
                    .description
                    .clone()
                    .unwrap_or_else(|| "Runs a jq program over the call arguments and returns its output.".to_string()),
                input_schema: parsed.effective_args_schema(),
            },
            Err(e) => ToolDescriptor {
                name: "jq".to_string(),
                description: format!("invalid jq tool spec: {e}"),
                input_schema: json!({"type": "object"}),
            },
        }
    }

    fn declared_outputs(&self, spec: &Value) -> Vec<OutputDecl> {
        parse_spec(spec).map(|p| p.outputs).unwrap_or_default()
    }

    fn payload_from_call_result<'a>(&self, call_result: &'a Value) -> Option<&'a Value> {
        Some(call_result)
    }

    fn requested_timeout(&self, spec: &Value) -> Option<Duration> {
        parse_spec(spec).ok().map(|p| Duration::from_secs(p.timeout_s) + OUTER_SLACK)
    }

    async fn call(&self, spec: &Value, args: Value, ctx: &CallCtx) -> Result<Value, KindError> {
        let parsed = parse_spec(spec)?;
        let validator = jsonschema::validator_for(&parsed.effective_args_schema())
            .map_err(|e| KindError::InvalidSpec(format!("args_schema: {e}")))?;
        if let Err(e) = validator.validate(&args) {
            let args_err = super::describe_args_error(&e);
            let mut data = args_err.data();
            if let Some(obj) = data.as_object_mut() {
                obj.insert("schema_path".to_string(), json!(e.schema_path.to_string()));
            }
            return Err(KindError::structured_with("args_invalid", args_err.message(), data));
        }
        let filter = compiled(&parsed.program)?;

        let budget = Duration::from_secs(parsed.timeout_s).min(ctx.time_remaining());
        let started = Instant::now();
        let deadline = started + budget;
        let cancel = Arc::new(AtomicBool::new(false));
        let (tx, rx) = tokio::sync::oneshot::channel();
        let prev = ctx.prev_result.clone().unwrap_or(Value::Null);
        let max_bytes = parsed.max_output_bytes;
        let thread_cancel = cancel.clone();
        std::thread::Builder::new()
            .name("jq-eval".into())
            .stack_size(EVALUATOR_STACK_BYTES)
            .spawn(move || {
                let _ = tx.send(run_filter(&filter, args, prev, &Limits { deadline, max_bytes }, &thread_cancel));
            })
            .map_err(|e| KindError::Exec(format!("starting the jq evaluator: {e}")))?;

        let finished = match tokio::time::timeout(budget + GUARD_SLACK, rx).await {
            Ok(Ok(finished)) => finished,
            Ok(Err(_)) => Finished::Failed("the jq evaluator ended without a result".to_string()),
            Err(_) => {
                cancel.store(true, Ordering::Relaxed);
                ABANDONED.fetch_add(1, Ordering::Relaxed);
                Finished::Timeout
            }
        };
        ctx.resources.record(started.elapsed().as_millis() as i64, 0);

        let result = match finished {
            Finished::Outputs(mut outputs) => Ok(if outputs.len() == 1 { outputs.remove(0) } else { Value::Array(outputs) }),
            Finished::TooLarge { bytes } => Err(KindError::structured_with(
                codes::TOOL_OUTPUT_TOO_LARGE,
                format!("the program's output is {bytes} bytes, over its max_output_bytes of {}", parsed.max_output_bytes),
                json!({"max_output_bytes": parsed.max_output_bytes, "output_bytes": bytes}),
            )),
            Finished::Timeout => Err(KindError::structured_with(
                codes::TOOL_TIMEOUT,
                format!("the program ran past its timeout_s of {}", parsed.timeout_s),
                json!({"timeout_s": parsed.timeout_s}),
            )),
            Finished::Failed(message) => {
                let message = jq_style(&message);
                Err(KindError::structured_with(
                    codes::TOOL_EXCEPTION,
                    message.clone(),
                    json!({
                        "phase": key::PHASE_TOOL_CODE,
                        "exception_class": "jq_error",
                        "message": message,
                        "source_line": first_line(&parsed.program),
                        "source_column": Value::Null,
                        "position_known": false,
                    }),
                ))
            }
        };
        result.map_err(super::contract::tag_families)
    }

    fn example(&self) -> KindExample {
        super::docs::parse_kind_doc(include_str!("../../docs/kinds/jq.md"))
    }
}
