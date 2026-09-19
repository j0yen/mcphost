//! The `wasm` kind (PRD-mcphost-wasm-kind): `spec` carries a compiled
//! WebAssembly *component* (component-model, not a bare core module),
//! executed in-process under Wasmtime with a per-call fuel budget, a memory
//! cap, and a wall-time deadline enforced by the runtime itself. Isolation
//! and cold-start speed come from the compiled binary and the embedded
//! runtime, not from the host's userns/bwrap/AppArmor policy the `python`
//! kind's sandbox needs (the vision's cycle-53 caveat this PRD answers) --
//! this kind never shells out to `bwrap`/`unshare`/any external sandbox
//! binary, so it stays available even when [`crate::sandbox`] reports no
//! mechanism at all (requirement 5 / AC4).
//!
//! ## Component contract
//!
//! Every component this kind calls must export exactly one function
//! (see `tests/fixtures/wasm-src/*/wit/world.wit` for the WIT source the
//! test fixtures are built from):
//!
//! ```wit
//! call: func(args: string) -> result<string, string>
//! ```
//!
//! `args` is the call's JSON-encoded arguments; a returned `ok(s)` is
//! parsed as JSON and becomes `result.payload` (declared `outputs` are
//! promoted into it with the same [`super::apply_output_decls`] semantics
//! `python`'s own promotion uses -- AC10); a returned `err(s)` becomes a
//! structured `tool_exception` naming `s`. No WASI is linked in -- non-goal
//! 4 scopes this slice to pure compute over the call arguments and result,
//! so a conforming component needs no host imports at all.
//!
//! ## Limits
//!
//! A component that runs past its fuel budget or wall-time deadline is
//! interrupted and reported as `tool_timeout`; one that tries to grow its
//! linear memory past `memory_mb` is interrupted and reported as
//! `tool_oom`; any other trap (an explicit `unreachable`, an out-of-bounds
//! access, ...) is reported as `tool_trapped` with the trap's message
//! logged to `host.tool_logs`, never as a raw panic (AC3, AC6).
//!
//! ## Module cache
//!
//! Compiling a component (cranelift codegen) is the expensive part of
//! running one; instantiation is milliseconds. [`WasmKind`] caches the
//! compiled [`Component`] per `(tenant_id, tool_name)`, keyed additionally
//! by a hash of the component bytes so a republish's new bytes always miss
//! -- and `on_tool_changed`/`on_tool_published` evict the old entry
//! outright the moment a publish/republish/remove happens, rather than
//! relying on the hash check alone (AC8).

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use base64::Engine as _;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use wasmtime::component::{Component, Linker};
use wasmtime::{Config, Engine, ResourceLimiter, Store};

use super::{CallCtx, Kind, KindError, KindExample, OutputDecl, ToolDescriptor};

/// Recorded in `/healthz` beside `sandbox_mechanism` (P1 requirement 8 /
/// AC9). Kept in sync with the `wasmtime` version pinned in `Cargo.toml`.
pub const WASM_RUNTIME_VERSION: &str = "wasmtime 38.0.4";

/// Open question resolved at build: a flat cap, not a per-plan one --
/// mirrors `python`'s own `MAX_SOURCE_BYTES` (also a flat constant, not
/// read from `Plan`). 47 KiB, not a rounder 32 or 64 KiB, because a real
/// component built by the expected toolchain (`cargo component build`
/// against `wasm32-wasip1`) that touches the heap at all links the full
/// `wasi:cli` "command" world -- tens of KB of adapter code -- regardless
/// of how little the guest's own logic does; this is the smallest round
/// bound that comfortably fits one (this kind's own `oom` test fixture is
/// ~47 KiB) while staying under `MAX_SPEC_BYTES` (64 KiB) once base64 and
/// the surrounding spec JSON overhead are accounted for, so this kind's own
/// "over the bound" rejection is still the one an oversize publish actually
/// hits rather than the generic `spec_too_large` check winning the race.
const MAX_COMPONENT_BYTES: usize = 47 * 1024;
const MAX_TIMEOUT_S: u64 = 30;
const DEFAULT_TIMEOUT_S: u64 = 5;
const MAX_MEMORY_MB: u64 = 256;
const DEFAULT_MEMORY_MB: u64 = 64;
/// A generous CPU bound independent of wall time -- large enough that no
/// legitimate millisecond-scale call gets anywhere near it, small enough
/// that a genuinely runaway component doesn't have to wait out the wall
/// clock either.
const FUEL_BUDGET: u64 = 4_000_000_000;
/// The internal wall-time cutoff fires at this fraction of the effective
/// timeout `requested_timeout` also reports to `handler.rs`'s own
/// `tokio::time::timeout` -- strictly earlier, so this kind's own structured
/// `tool_timeout` always wins the race and the spawned execution thread is
/// actually stopped, rather than merely abandoned by the outer timeout
/// while it keeps spinning on the blocking thread pool.
const WALL_BUDGET_SLACK: f64 = 0.8;

#[derive(Debug, Clone)]
struct WasmSpec {
    component: Vec<u8>,
    args_schema: Option<Value>,
    outputs: Vec<OutputDecl>,
    timeout_s: Option<u64>,
    memory_mb: Option<u64>,
}

impl WasmSpec {
    fn effective_args_schema(&self) -> Value {
        self.args_schema
            .clone()
            .unwrap_or_else(|| json!({"type": "object"}))
    }

    fn effective_timeout(&self) -> Duration {
        Duration::from_secs(self.timeout_s.unwrap_or(DEFAULT_TIMEOUT_S))
    }

    fn effective_memory_bytes(&self) -> usize {
        (self.memory_mb.unwrap_or(DEFAULT_MEMORY_MB) as usize) * 1024 * 1024
    }
}

fn parse_spec(spec: &Value) -> Result<WasmSpec, KindError> {
    if !spec.is_object() {
        return Err(KindError::InvalidSpec("spec: must be a JSON object".into()));
    }
    let b64 = spec.get("component").and_then(Value::as_str).ok_or_else(|| {
        KindError::InvalidSpec(
            "component: is required (base64-encoded WebAssembly component)".into(),
        )
    })?;
    let component = base64::engine::general_purpose::STANDARD
        .decode(b64)
        .map_err(|e| KindError::InvalidSpec(format!("component: not valid base64: {e}")))?;

    let args_schema = match spec.get("args_schema") {
        None | Some(Value::Null) => None,
        Some(schema) => {
            jsonschema::validator_for(schema).map_err(|e| {
                KindError::InvalidSpec(format!("args_schema: not a valid JSON Schema: {e}"))
            })?;
            Some(schema.clone())
        }
    };
    let outputs = match spec.get("outputs") {
        None => Vec::new(),
        Some(v) => super::normalize_outputs(v)?,
    };
    let timeout_s = match spec.get("timeout_s") {
        None | Some(Value::Null) => None,
        Some(v) => Some(v.as_u64().ok_or_else(|| {
            KindError::InvalidSpec("timeout_s: must be a positive integer".into())
        })?),
    };
    let memory_mb = match spec.get("memory_mb") {
        None | Some(Value::Null) => None,
        Some(v) => Some(v.as_u64().ok_or_else(|| {
            KindError::InvalidSpec("memory_mb: must be a positive integer".into())
        })?),
    };

    Ok(WasmSpec {
        component,
        args_schema,
        outputs,
        timeout_s,
        memory_mb,
    })
}

fn component_error(e: impl std::fmt::Display) -> KindError {
    KindError::InvalidSpec(format!(
        "component: not a valid WebAssembly component (requires the component model): {e}"
    ))
}

/// Requirement 1 / AC2: every simultaneously-failing field, same convention
/// as `python`'s `validate_spec_fields_all`. The size cap is checked before
/// ever attempting to compile -- a too-large payload is refused without
/// spending cranelift time on it, and never confused with "not a
/// component" (a component just over the cap is refused for its size, not
/// re-diagnosed as malformed).
fn validate_spec_fields_all(parsed: &WasmSpec, engine: &Engine) -> Vec<KindError> {
    let mut errors = Vec::new();
    if parsed.component.len() > MAX_COMPONENT_BYTES {
        errors.push(KindError::InvalidSpec(format!(
            "component: {} bytes, over the {MAX_COMPONENT_BYTES}-byte limit",
            parsed.component.len()
        )));
    } else if let Err(e) = Component::new(engine, &parsed.component) {
        errors.push(component_error(e));
    }
    if let Some(t) = parsed.timeout_s
        && !(1..=MAX_TIMEOUT_S).contains(&t)
    {
        errors.push(KindError::InvalidSpec(format!(
            "timeout_s: must be between 1 and {MAX_TIMEOUT_S}; got {t}"
        )));
    }
    if let Some(m) = parsed.memory_mb
        && !(1..=MAX_MEMORY_MB).contains(&m)
    {
        errors.push(KindError::InvalidSpec(format!(
            "memory_mb: must be between 1 and {MAX_MEMORY_MB}; got {m}"
        )));
    }
    errors
}

/// Requirement 3 / AC1, AC10: wraps the component's parsed JSON return
/// value as `{"payload": ...}` -- every wasm call's output lands at
/// `result.payload` regardless of whether `outputs` declares anything, so
/// AC1's plain echo case satisfies the envelope contract on its own. An
/// object return keeps its original top-level fields and gets `payload`
/// added alongside them (not wrapped underneath), the exact same shape
/// python's own `apply_declared_outputs` produces for an object return.
/// Declared fields are promoted into that same `payload` with
/// [`super::apply_output_decls`]'s any-wrapper search (`wrapper_keys:
/// None`), the same search python's own `apply_declared_outputs` uses --
/// AC10's "identically to the python kind."
fn build_result(source: Value, declared: &[OutputDecl]) -> Value {
    match source {
        Value::Object(obj) => {
            let source_val = Value::Object(obj.clone());
            let mut payload_map = obj.clone();
            if !declared.is_empty() {
                super::apply_output_decls(&mut payload_map, &source_val, declared, None);
            }
            let mut out = obj;
            out.insert("payload".to_string(), Value::Object(payload_map));
            Value::Object(out)
        }
        other => {
            if declared.is_empty() {
                json!({ "payload": other })
            } else {
                let mut payload_map = Map::new();
                payload_map.insert(declared[0].name.clone(), other);
                json!({ "payload": Value::Object(payload_map) })
            }
        }
    }
}

/// A component's `call` returned normally: either its own `ok` (a
/// JSON-encoded result string) or its own `err` (a plain message, not a
/// trap -- promoted to `tool_exception`, distinct from `tool_trapped`).
enum RunOutcome {
    Ok(String),
    Err(String),
}

/// A component's `call` did not return normally.
enum RunError {
    Timeout,
    Oom,
    Trap(String),
}

/// Distinguishes "the memory limiter refused a `memory.grow`" from every
/// other trap, without needing to pattern-match wasmtime's own `Trap`
/// enum (which has no memory-limiter-specific variant -- a host-injected
/// `ResourceLimiter` error is reported as an ordinary `anyhow::Error` whose
/// chain this type is threaded through, per `ResourceLimiter::memory_growing`'s
/// own doc: "get a precise backtrace at what requested so much memory").
#[derive(Debug)]
struct MemoryCapExceeded;

impl std::fmt::Display for MemoryCapExceeded {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "component exceeded its memory cap")
    }
}

impl std::error::Error for MemoryCapExceeded {}

/// The `Store<T>` data: the resource limiter this call runs under, plus the
/// minimal WASI context a guest built by the expected toolchain needs to
/// even instantiate (see `Cargo.toml`'s `wasmtime-wasi` comment). No
/// preopened directories and no inherited network -- filesystem/socket
/// imports are wired into the linker (`wasmtime_wasi::p2::add_to_linker_sync`
/// links the whole `wasi:cli` world) but every operation on them fails or is
/// simply never reachable, since nothing here grants access. `peak_bytes` is
/// reported back for metering even though this slice doesn't enforce
/// anything from it -- observability, not a limit.
struct GuestState {
    max_bytes: usize,
    peak_bytes: usize,
    wasi_ctx: wasmtime_wasi::WasiCtx,
    table: wasmtime_wasi::ResourceTable,
}

impl ResourceLimiter for GuestState {
    fn memory_growing(
        &mut self,
        _current: usize,
        desired: usize,
        _maximum: Option<usize>,
    ) -> anyhow::Result<bool> {
        if desired > self.peak_bytes {
            self.peak_bytes = desired;
        }
        if desired > self.max_bytes {
            return Err(anyhow::Error::new(MemoryCapExceeded));
        }
        Ok(true)
    }

    fn table_growing(
        &mut self,
        _current: usize,
        _desired: usize,
        _maximum: Option<usize>,
    ) -> anyhow::Result<bool> {
        Ok(true)
    }
}

impl wasmtime_wasi::WasiView for GuestState {
    fn ctx(&mut self) -> wasmtime_wasi::WasiCtxView<'_> {
        wasmtime_wasi::WasiCtxView {
            ctx: &mut self.wasi_ctx,
            table: &mut self.table,
        }
    }
}

fn classify_error(err: anyhow::Error) -> RunError {
    if err
        .chain()
        .any(|c| c.downcast_ref::<MemoryCapExceeded>().is_some())
    {
        return RunError::Oom;
    }
    if let Some(trap) = err.downcast_ref::<wasmtime::Trap>()
        && matches!(trap, wasmtime::Trap::OutOfFuel | wasmtime::Trap::Interrupt)
    {
        return RunError::Timeout;
    }
    RunError::Trap(err.to_string())
}

/// Runs one call, synchronously, in whatever thread it's given (the caller
/// is expected to run this inside `spawn_blocking` -- wasm execution is
/// CPU-bound and must not block the async executor). Returns the peak
/// memory this call's linear memory reached, the guest's captured stderr
/// (best-effort, lossy UTF-8, for `host.tool_logs`), and the outcome.
fn run_component(
    engine: &Engine,
    component: &Component,
    memory_cap: usize,
    fuel_budget: u64,
    args_json: &str,
) -> (usize, String, Result<RunOutcome, RunError>) {
    let stderr = wasmtime_wasi::p2::pipe::MemoryOutputPipe::new(64 * 1024);
    let wasi_ctx = wasmtime_wasi::WasiCtxBuilder::new().stderr(stderr.clone()).build();
    let state = GuestState {
        max_bytes: memory_cap,
        peak_bytes: 0,
        wasi_ctx,
        table: wasmtime_wasi::ResourceTable::new(),
    };
    let mut store = Store::new(engine, state);
    store.limiter(|state| state as &mut dyn ResourceLimiter);
    store.set_epoch_deadline(1);

    let outcome = (|| -> anyhow::Result<RunOutcome> {
        store.set_fuel(fuel_budget)?;
        let mut linker: Linker<GuestState> = Linker::new(engine);
        wasmtime_wasi::p2::add_to_linker_sync(&mut linker)?;
        let instance = linker.instantiate(&mut store, component)?;
        let func = instance
            .get_typed_func::<(String,), (Result<String, String>,)>(&mut store, "call")?;
        let (result,) = func.call(&mut store, (args_json.to_string(),))?;
        Ok(match result {
            Ok(s) => RunOutcome::Ok(s),
            Err(e) => RunOutcome::Err(e),
        })
    })();

    let peak_bytes = store.data().peak_bytes;
    let stderr_text = String::from_utf8_lossy(&stderr.contents()).into_owned();
    (peak_bytes, stderr_text, outcome.map_err(classify_error))
}

struct CacheEntry {
    hash: [u8; 32],
    component: Arc<Component>,
}

pub struct WasmKind {
    engine: Engine,
    cache: Mutex<HashMap<(i64, String), CacheEntry>>,
    compiles: AtomicU64,
    cache_hits: AtomicU64,
}

impl Default for WasmKind {
    fn default() -> Self {
        Self::new()
    }
}

impl WasmKind {
    pub fn new() -> Self {
        let mut config = Config::new();
        config.consume_fuel(true);
        config.epoch_interruption(true);
        let engine = Engine::new(&config)
            .expect("wasmtime engine config (fuel + epoch interruption) is always valid");
        Self {
            engine,
            cache: Mutex::new(HashMap::new()),
            compiles: AtomicU64::new(0),
            cache_hits: AtomicU64::new(0),
        }
    }

    /// AC8: the number of times this kind has actually compiled a
    /// component (cranelift codegen), as opposed to reusing a cached one.
    pub fn compile_count(&self) -> u64 {
        self.compiles.load(Ordering::Relaxed)
    }

    /// AC8: the number of calls that reused a cached compiled artifact.
    pub fn cache_hit_count(&self) -> u64 {
        self.cache_hits.load(Ordering::Relaxed)
    }

    fn compile(&self, bytes: &[u8]) -> Result<Component, KindError> {
        self.compiles.fetch_add(1, Ordering::Relaxed);
        Component::new(&self.engine, bytes).map_err(component_error)
    }

    /// Requirement 7 / AC8: reuses the cached compiled [`Component`] for
    /// `(tenant_id, tool_name)` when its content hash still matches
    /// `bytes` -- a republish's new bytes (or a fresh hash after
    /// `on_tool_changed`/`on_tool_published` already evicted the old entry)
    /// always recompiles. Calls with no stable tool name (`host.spec_test`,
    /// the conformance suite, `CallCtx::for_test`) always compile fresh --
    /// there is no name to key a cache entry on.
    fn get_or_compile(
        &self,
        tenant_id: i64,
        tool_name: Option<&str>,
        bytes: &[u8],
    ) -> Result<Arc<Component>, KindError> {
        let hash: [u8; 32] = Sha256::digest(bytes).into();
        let Some(name) = tool_name else {
            return Ok(Arc::new(self.compile(bytes)?));
        };
        let key = (tenant_id, name.to_string());
        if let Some(entry) = self.cache.lock().unwrap().get(&key)
            && entry.hash == hash
        {
            self.cache_hits.fetch_add(1, Ordering::Relaxed);
            return Ok(entry.component.clone());
        }
        let component = Arc::new(self.compile(bytes)?);
        self.cache.lock().unwrap().insert(
            key,
            CacheEntry {
                hash,
                component: component.clone(),
            },
        );
        Ok(component)
    }
}

#[async_trait::async_trait]
impl Kind for WasmKind {
    fn name(&self) -> &'static str {
        "wasm"
    }

    fn validate(&self, spec: &Value) -> Result<(), KindError> {
        self.validate_all(spec).into_iter().next().map_or(Ok(()), Err)
    }

    fn validate_all(&self, spec: &Value) -> Vec<KindError> {
        match parse_spec(spec) {
            Ok(parsed) => validate_spec_fields_all(&parsed, &self.engine),
            Err(e) => vec![e],
        }
    }

    fn describe(&self, spec: &Value) -> ToolDescriptor {
        match parse_spec(spec) {
            Ok(parsed) => ToolDescriptor {
                name: "wasm".to_string(),
                description: "Calls a compiled WebAssembly component and returns its result."
                    .to_string(),
                input_schema: parsed.effective_args_schema(),
            },
            Err(e) => ToolDescriptor {
                name: "wasm".to_string(),
                description: format!("invalid wasm tool spec: {e}"),
                input_schema: json!({"type": "object"}),
            },
        }
    }

    fn declared_outputs(&self, spec: &Value) -> Vec<OutputDecl> {
        parse_spec(spec).map(|p| p.outputs).unwrap_or_default()
    }

    fn payload_from_call_result<'a>(&self, call_result: &'a Value) -> Option<&'a Value> {
        call_result.get("payload")
    }

    fn source_for_output_search<'a>(&self, call_result: &'a Value) -> Option<&'a Value> {
        call_result.get("payload")
    }

    /// Requirement 1: a spec's own `timeout_s` (bounded, defaulted) drives
    /// both the host's generic per-call deadline (`handler.rs`'s
    /// `resolve_call_timeout`) and this kind's own internal wall-time
    /// cutoff (`WALL_BUDGET_SLACK` of the same duration) -- see that
    /// constant's doc for why the two must agree.
    fn requested_timeout(&self, spec: &Value) -> Option<Duration> {
        parse_spec(spec).ok().map(|p| p.effective_timeout())
    }

    async fn call(&self, spec: &Value, args: Value, ctx: &CallCtx) -> Result<Value, KindError> {
        let parsed = parse_spec(spec)?;

        // Defensive re-validation, same rationale as `http`/`python`'s own
        // `call`: the host's generic dispatch already checks `args` against
        // `describe()`'s schema, but a direct caller (the conformance
        // suite, `CallCtx::for_test`) may not have.
        let effective_schema = parsed.effective_args_schema();
        let validator = jsonschema::validator_for(&effective_schema)
            .map_err(|e| KindError::InvalidSpec(format!("args_schema: {e}")))?;
        if let Err(e) = validator.validate(&args) {
            let data = json!({
                "schema_path": e.schema_path.to_string(),
                "instance_path": e.instance_path.to_string(),
            });
            return Err(KindError::structured_with("args_invalid", e.to_string(), data));
        }

        let component =
            self.get_or_compile(ctx.tenant_id, ctx.tool_name.as_deref(), &parsed.component)?;
        let args_json = serde_json::to_string(&args)
            .map_err(|e| KindError::Exec(format!("serializing call arguments: {e}")))?;

        let engine = self.engine.clone();
        let memory_cap = parsed.effective_memory_bytes();
        let wall_budget = parsed.effective_timeout().mul_f64(WALL_BUDGET_SLACK);

        // Requirement 2: the wall-time half of the budget -- increments the
        // engine's epoch after `wall_budget` elapses, tripping the
        // cranelift-inserted epoch check the running component will hit at
        // its next loop back-edge/call, however tight the loop. Aborted the
        // instant the blocking call returns on its own, so a fast call
        // never waits on this timer.
        let deadline_engine = engine.clone();
        let deadline_task = tokio::spawn(async move {
            tokio::time::sleep(wall_budget).await;
            deadline_engine.increment_epoch();
        });

        let started = Instant::now();
        let run = tokio::task::spawn_blocking(move || {
            run_component(&engine, &component, memory_cap, FUEL_BUDGET, &args_json)
        });
        let (peak_bytes, stderr_text, outcome) = run
            .await
            .map_err(|e| KindError::Exec(format!("wasm execution task failed: {e}")))?;
        deadline_task.abort();
        let elapsed_ms = started.elapsed().as_millis() as i64;

        ctx.resources.record(elapsed_ms, (peak_bytes / 1024) as i64);
        if !stderr_text.trim().is_empty() {
            ctx.log.log(&format!("stderr: {}", stderr_text.trim_end()));
        }

        match outcome {
            Ok(RunOutcome::Ok(json_str)) => {
                let value: Value =
                    serde_json::from_str(&json_str).unwrap_or(Value::String(json_str));
                Ok(build_result(value, &parsed.outputs))
            }
            Ok(RunOutcome::Err(message)) => Err(KindError::structured("tool_exception", message)),
            Err(RunError::Timeout) => Err(KindError::structured(
                "tool_timeout",
                "the call exceeded its timeout",
            )),
            Err(RunError::Oom) => Err(KindError::structured(
                "tool_oom",
                "the call exceeded its memory limit",
            )),
            Err(RunError::Trap(message)) => {
                // AC6: the trap message lands in host.tool_logs, and the
                // call error is structured (not a raw panic/exec error).
                ctx.log.log(&format!("trap: {message}"));
                Err(KindError::structured_with(
                    "tool_trapped",
                    "the component trapped",
                    json!({"trap": message}),
                ))
            }
        }
    }

    /// AC3: a republish/remove evicts this tool's cached compiled
    /// component outright, rather than relying only on the content-hash
    /// check to notice the change on the next call.
    async fn on_tool_changed(&self, tenant_id: i64, local_name: &str) {
        self.cache.lock().unwrap().remove(&(tenant_id, local_name.to_string()));
    }

    async fn on_tool_published(
        &self,
        tenant_id: i64,
        _namespace: &str,
        local_name: &str,
        _spec: &Value,
    ) {
        self.cache.lock().unwrap().remove(&(tenant_id, local_name.to_string()));
    }

    async fn on_tenant_removed(&self, tenant_id: i64) {
        self.cache.lock().unwrap().retain(|(t, _), _| *t != tenant_id);
    }

    fn example(&self) -> KindExample {
        super::docs::parse_kind_doc(include_str!("../../docs/kinds/wasm.md"))
    }
}
