//! The `python` kind: a tenant publishes one Python source file defining
//! `main(args: dict) -> dict`, an optional PyPI dependency list and an
//! `args_schema`; the host builds an isolated `uv`-managed virtualenv once
//! per distinct requirement set and runs every call in a fresh sandboxed
//! subprocess via `crate::sandbox`. PRD: `PRD-mcphost-code-tools.md`.
//!
//! ## Scoped decisions (documented here rather than silently assumed)
//!
//! * **Env directory keying**: the PRD's example path is
//!   `envs/<tenant>/<tool>/<hash>`, but [`Kind::call`]'s signature carries
//!   the tenant (via [`CallCtx`]) and *not* the tool's local name, and the
//!   PRD's own goal text says environments "are shared by identical
//!   requirement sets within a tenant" -- which the `<tool>` path segment
//!   would actually prevent. This implementation keys by
//!   `envs/<namespace>/<hash-of-requirements>` only, matching the stated
//!   sharing goal and what the trait can actually see.
//! * **Build trigger**: rather than a separate publish-time hook (the
//!   `Kind` trait has none, and [`Kind::validate_async`] doesn't carry
//!   tenant/tool context either), a build starts on the *first call* that
//!   finds no ready/failed environment on disk, which returns `tool_building`
//!   immediately without waiting -- satisfying AC2's "a call during the
//!   build returns `tool_building`" literally, since that first call *is*
//!   during the build it just started.
//! * **`network: public`**: no SSRF-filtering egress proxy ships with this
//!   PRD (see `crate::sandbox::NetworkMode` docs); `network: public` grants
//!   full outbound access unless `$MCPHOST_EGRESS_PROXY` is set. No
//!   acceptance criterion exercises this path.
//! * **Per-tenant CPU budget** (requirement 8's 600 CPU-seconds/hour) is
//!   enforced with an in-memory, fixed, non-admin-configurable window
//!   (resets hourly); the `admin.tenant_limits` configuration RPC the PRD
//!   also names is not implemented -- no acceptance criterion requires it,
//!   only the budget's *effect* (AC14).
//!
//! ## PRD-mcphost-code-tools-warm-pool
//!
//! * **Pool key**: [`Kind::call`] still carries no tool name, so warm-pool
//!   keying leans on the additive [`CallCtx::tool_name`] field this PRD adds
//!   (populated by every real dispatch path in `handler.rs`) rather than
//!   the source/requirements hash alone -- `on_tool_changed` (republish/
//!   remove) needs the *name* to evict synchronously (AC3), which a
//!   content hash alone can't answer without also being told the old
//!   content.
//! * **Warm-pool `RLIMIT_CPU`**: `ResourceLimits::cpu_seconds` is a kernel
//!   rlimit on the *process's whole lifetime*, not resettable per call --
//!   applying a single call's `timeout_s` to a sandbox meant to serve many
//!   calls over its TTL would starve it after a handful of cheap calls.
//!   A warm sandbox's rlimit is `timeout_s * 1000` (generous enough for its
//!   bounded TTL/lifetime; effectively "the kernel-level backstop, not the
//!   real limit") -- per-call enforcement is the wall-clock read timeout in
//!   `sandbox::PersistentSandbox::call`, matching a cold call's own
//!   wall-clock-cap-over-rlimit relationship (`wall_clock_timeout` is
//!   already `timeout_s + 2s`, longer than `RLIMIT_CPU`'s `timeout_s`).
//! * **Metrics (requirement 4)**: `warm_hits`/`warm_misses`/`pool_size` are
//!   tracked in-memory on [`WarmPool`] (proven by this module's own unit
//!   tests) but not surfaced through `host.usage`/`admin.usage` this tick --
//!   both are cross-kind, `AppState`-level aggregations today, and piping a
//!   `python`-specific counter through them needs a concretely-typed handle
//!   `AppState` doesn't currently keep (only `Arc<dyn Kind>`). No acceptance
//!   criterion in this PRD names the RPC surface, only the pool's behavior
//!   (AC1-5, AC8); wiring the counters into `host.usage` is left for the
//!   PRD that actually needs an operator-visible number.
//! * **Requirement 6 / AC8 (pre-warm on publish, P1) is not implemented**
//!   this tick. A warm sandbox bakes its `SECRET_*` environment variables in
//!   at spawn time (see [`call_fingerprint`]'s doc), and resolving a
//!   tenant's secrets needs `AppState`/`state.secrets` plus a DB round trip
//!   -- machinery `control::tool_publish` (where a publish-time hook would
//!   fire) doesn't currently have wired to any `Kind`, unlike `handler.rs`'s
//!   real call paths (`build_secret_resolver`). Pre-warming only
//!   secret-free tools would be an easy partial version, but risks the
//!   opposite bug it would need to get right on day one: a pool entry
//!   spawned without secrets that a *secret-using* publish's first real
//!   call could wrongly appear to match on source/requirements alone if
//!   secrets were ever left out of the fingerprint (they are not -- see
//!   `call_fingerprint` -- but a pre-warm path that never has secrets to
//!   fold in would always produce the empty-secret-set fingerprint, silently
//!   correct only by coincidence). No P0 acceptance criterion needs this;
//!   AC1's "second call is fast" already holds via the cold-call
//!   `maybe_promote_to_warm` path.
//! * **`host.tool_run` execution path**: rather than overloading
//!   [`Kind::call`] with a `dry_run` flag on [`CallCtx`] (the technical
//!   consideration's suggestion), this ships as its own [`Kind::tool_run`]
//!   trait method with its own response shape
//!   (`{stdout, stderr, exit_code, result}`) -- `call`'s contract (a bare
//!   result `Value` or a [`KindError`]) has no room for raw stdout/stderr/
//!   exit code without a breaking change to every other kind's return type.
//!   `host.tool_run` always runs cold (never touches the warm pool): it is
//!   the "why did my tool print nothing" debug path, not a throughput path,
//!   and keeping it off the pool means a debug run can never evict or block
//!   on a warm sandbox serving real traffic.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use serde::Deserialize;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use tokio::sync::Semaphore;

use super::infer;
use super::{CallCtx, Kind, KindError, KindExample, ToolDescriptor};
use crate::sandbox::{
    self, IsolationMechanism, NetworkMode, PersistentCallOutcome, PersistentSandbox,
    ResourceLimits, SandboxOutcome,
};

// ---- limits & defaults (requirement 1) -------------------------------------

const MAX_SOURCE_BYTES: usize = 64 * 1024;
const MAX_REQUIREMENTS: usize = 20;
const DEFAULT_TIMEOUT_S: u64 = 10;
const MAX_TIMEOUT_S: u64 = 60;
const DEFAULT_MEMORY_MB: u64 = 256;
const MAX_MEMORY_MB: u64 = 1024;
const MAX_OPEN_FILES: u64 = 64;
const MAX_FILE_SIZE_MB: u64 = 16;
const BUILD_TIMEOUT: Duration = Duration::from_secs(120);
const DEFAULT_CPU_BUDGET_MS_PER_HOUR: u64 = 600_000;
const CPU_BUDGET_WINDOW: Duration = Duration::from_secs(3600);
const DEFAULT_MAX_CONCURRENT_CALLS: usize = 20;

// ---- warm pool (PRD-mcphost-code-tools-warm-pool requirement 1) -----------

/// `MCPHOST_PY_WARM_TTL` default (seconds): how long an idle warm sandbox
/// survives before the reaper kills it.
const DEFAULT_WARM_TTL_S: u64 = 60;
/// `MCPHOST_PY_WARM_PER_TENANT` default: at most this many warm sandboxes
/// for any one tenant, across all of its tools, at once.
const DEFAULT_WARM_PER_TENANT: usize = 2;
/// `MCPHOST_PY_WARM_MAX` default: at most this many warm sandboxes box-wide.
const DEFAULT_WARM_MAX: usize = 16;
/// How often the reaper task wakes to check every entry's TTL.
const WARM_REAP_INTERVAL: Duration = Duration::from_secs(5);
/// `host.tool_run`'s stdout/stderr cap (requirement 3).
const TOOL_RUN_CAP_BYTES: usize = 64 * 1024;

// ---- spec -------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
struct PythonSpec {
    source: String,
    #[serde(default)]
    requirements: Vec<String>,
    /// Requirement 1: optional -- when absent, [`infer::infer_python_args_schema`]
    /// derives it from `source` (requirement 2). When present, used exactly
    /// as before with no inference performed.
    #[serde(default)]
    args_schema: Option<Value>,
    #[serde(default)]
    timeout_s: Option<u64>,
    #[serde(default)]
    memory_mb: Option<u64>,
    #[serde(default)]
    network: Option<String>,
    #[serde(default)]
    secrets: Vec<String>,
    #[serde(default)]
    description: Option<String>,
    /// PRD-mcphost-result-envelope-contract requirement 1: field names this
    /// tool's caller can expect to read at `result.payload.<field>`.
    /// Optional -- a spec that omits it (every spec published before this
    /// PRD) gets no envelope changes (Migration/compatibility: additive).
    #[serde(default)]
    outputs: Vec<String>,
}

impl PythonSpec {
    /// Requirement 1/2: the schema to validate calls against -- the
    /// author's own, unchanged, or one derived from `source` when absent.
    /// Deterministic (AC13): a pure function of `source` alone.
    fn effective_args_schema(&self) -> Result<Value, KindError> {
        match &self.args_schema {
            Some(schema) => Ok(schema.clone()),
            None => infer::infer_python_args_schema(&self.source),
        }
    }

    /// Requirement 5/6: the requirements to build the environment with --
    /// the author's own, unchanged, or ones derived from `source`'s
    /// top-level imports when absent/empty.
    fn effective_requirements(&self) -> Result<Vec<String>, KindError> {
        if self.requirements.is_empty() {
            infer::infer_python_requirements(&self.source)
        } else {
            Ok(self.requirements.clone())
        }
    }

    fn effective_timeout_s(&self) -> u64 {
        self.timeout_s.unwrap_or(DEFAULT_TIMEOUT_S)
    }

    fn effective_memory_mb(&self) -> u64 {
        self.memory_mb.unwrap_or(DEFAULT_MEMORY_MB)
    }

    fn effective_network(&self) -> &str {
        self.network.as_deref().unwrap_or("none")
    }
}

fn parse_spec(spec: &Value) -> Result<PythonSpec, KindError> {
    if !spec.is_object() {
        return Err(KindError::InvalidSpec("spec: must be a JSON object".into()));
    }
    serde_json::from_value(spec.clone()).map_err(|e| KindError::InvalidSpec(format!("spec: {e}")))
}

/// requirement 2: "no `requirements` entry uses a URL, path or VCS
/// reference". Not a full PEP 508 parser -- just enough to name the exact
/// disallowed forms the AC cares about (a URL, an absolute/relative path, a
/// `git+`/`name @ url` direct reference) without accepting a project name
/// that happens to contain a slash-like character used maliciously.
fn requirement_is_allowed(req: &str) -> bool {
    let r = req.trim();
    if r.is_empty() || r.len() > 200 {
        return false;
    }
    if r.contains("://") || r.contains('@') || r.starts_with("git+") {
        return false;
    }
    if r.starts_with('.') || r.starts_with('/') || r.starts_with('~') || r.contains('\\') {
        return false;
    }
    let name_end = r.find(|c: char| "<>=!~;[ ".contains(c)).unwrap_or(r.len());
    let name = &r[..name_end];
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
}

/// Requirement 3 / AC2: every `requirements` entry is checked independently
/// -- collects every disallowed entry instead of stopping at the first --
/// so [`PythonKind::validate_all`] can report them all in one rejection.
fn validate_requirements_all(reqs: &[String]) -> Vec<KindError> {
    let mut errors = Vec::new();
    if reqs.len() > MAX_REQUIREMENTS {
        errors.push(KindError::InvalidSpec(format!(
            "requirements: at most {MAX_REQUIREMENTS} entries; got {}",
            reqs.len()
        )));
    }
    for req in reqs {
        if !requirement_is_allowed(req) {
            errors.push(KindError::structured_with(
                "requirement_not_allowed",
                format!("requirements: '{req}' is not an allowed PyPI requirement"),
                json!({"requirement": req}),
            ));
        }
    }
    errors
}

/// Requirement 3 / AC2: same fields [`validate_spec_fields`] checks, but
/// collecting every violation instead of returning at the first with `?`.
fn validate_spec_fields_all(parsed: &PythonSpec) -> Vec<KindError> {
    let mut errors = Vec::new();
    if parsed.source.is_empty() {
        errors.push(KindError::InvalidSpec("source: must not be empty".into()));
    } else if parsed.source.len() > MAX_SOURCE_BYTES {
        errors.push(KindError::InvalidSpec(format!(
            "source: {} bytes, over the {MAX_SOURCE_BYTES}-byte limit",
            parsed.source.len()
        )));
    }
    errors.extend(validate_requirements_all(&parsed.requirements));
    // Requirement 1: an author-supplied schema is checked exactly as
    // before; an absent one is left to `validate_async` (and later,
    // `describe`/`call`) to infer -- inference needs the sandboxed AST
    // check to have run first (AC16), which this synchronous fn can't do.
    if let Some(schema) = &parsed.args_schema
        && let Err(e) = jsonschema::validator_for(schema)
    {
        errors.push(KindError::InvalidSpec(format!(
            "args_schema: not a valid JSON Schema: {e}"
        )));
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
    if let Some(n) = &parsed.network
        && n != "none"
        && n != "public"
    {
        errors.push(KindError::InvalidSpec(format!(
            "network: must be 'none' or 'public'; got '{n}'"
        )));
    }
    errors
}

fn validate_spec_fields(parsed: &PythonSpec) -> Result<(), KindError> {
    validate_spec_fields_all(parsed)
        .into_iter()
        .next()
        .map_or(Ok(()), Err)
}

// ---- publish-time source check, in the sandbox (requirement 2) ------------

const AST_CHECK_SCRIPT: &str = r#"
import ast, json, sys

src = sys.stdin.buffer.read().decode("utf-8", errors="replace")
try:
    tree = ast.parse(src)
except SyntaxError as e:
    print(json.dumps({"ok": False, "error": "SyntaxError", "message": str(e.msg), "line": e.lineno}))
    sys.exit(0)

has_main = any(isinstance(n, ast.FunctionDef) and n.name == "main" for n in tree.body)
if not has_main:
    print(json.dumps({"ok": False, "error": "NoMain", "message": "source does not define main(args)"}))
else:
    print(json.dumps({"ok": True}))
"#;

/// PRD-mcphost-sandbox-ready: what interpreter `build_ast_check_spec`
/// wires into the `RunSpec` it builds. `RealPython3` is the only variant
/// production ever uses (`ast_check`'s own call, and the startup/periodic
/// self-test's default) -- the other two exist purely so this crate's own
/// tests can simulate one specific sandbox failure mode (a broken
/// interpreter, or a wrapper binary that can't even be spawned) without
/// needing an actually-broken host. See `PythonKind::for_test_with_selftest`.
#[derive(Debug, Clone)]
enum SelftestInterpreter {
    RealPython3,
    /// Test-only: `body` is written as an executable shell script into the
    /// probe's own scratch dir (so it's visible inside the sandbox under
    /// every real isolation mechanism, `--tmpfs /tmp` included) and exec'd
    /// in place of `/usr/bin/python3`. `mechanism` on the resulting
    /// `SandboxStatus` is honestly whatever isolation mechanism is really in
    /// force (e.g. real `bwrap` on this repo's own build machine) -- only
    /// the interpreter's *behavior* is faked, letting a test reproduce
    /// e.g. the hub's exact `RTM_NEWADDR` stderr on a box where bwrap
    /// actually works.
    ///
    /// PRD-mcphost-classify-precision: gated the same way as the
    /// constructors that are this variant's only source (`Self::Fake` is
    /// built only in `set_selftest_interpreter_for_test`) -- an ungated
    /// variant is "constructed" nowhere in a default-features build, which
    /// `-D warnings` correctly flags as dead code.
    #[cfg(any(test, feature = "test-support"))]
    Fake(String),
    /// Test-only (AC9's "missing bwrap/unshare" case): a nonexistent
    /// interpreter path run with `IsolationMechanism::None` (no real
    /// wrapper spawned at all), so `sandbox::run` fails at
    /// `Command::spawn()` itself -- this crate cannot uninstall bwrap from
    /// a shared dev/CI box to reproduce a genuinely-missing wrapper binary,
    /// so this exercises the same spawn-level-`Err` code path a missing
    /// wrapper would take (`SandboxStatus::from_probe`'s `Err` arm).
    ///
    /// PRD-mcphost-classify-precision: gated the same way as `Fake` above.
    #[cfg(any(test, feature = "test-support"))]
    MissingBinarySpawnFailure,
}

/// Builds the `RunSpec` for an ast-check of `source`, shared by [`ast_check`]
/// (the real publish-time check) and the sandbox self-test
/// (`PythonKind::selftest_probe`) -- PRD-mcphost-sandbox-ready requirement 1:
/// "the self-test must call `sandbox::run` with a `RunSpec` built by the
/// same function `ast_check` uses, so a divergence between 'probe passed'
/// and 'publish failed' cannot exist." Returns the scratch dir alongside the
/// spec so the caller can clean it up once the run completes.
async fn build_ast_check_spec(
    source: &str,
    isolation: IsolationMechanism,
    interpreter: &SelftestInterpreter,
) -> std::io::Result<(PathBuf, sandbox::RunSpec)> {
    let scratch = std::env::temp_dir().join(format!(
        "mcphost-astcheck-{}-{}",
        std::process::id(),
        rand::random::<u64>()
    ));
    tokio::fs::create_dir_all(&scratch).await?;
    let script_path = scratch.join("ast_check.py");
    tokio::fs::write(&script_path, AST_CHECK_SCRIPT).await?;

    let (bin, args, isolation, read_only_dirs) = match interpreter {
        SelftestInterpreter::RealPython3 => (
            PathBuf::from("/usr/bin/python3"),
            vec!["-I".to_string(), "-S".to_string()],
            isolation,
            system_python_dirs(),
        ),
        #[cfg(any(test, feature = "test-support"))]
        SelftestInterpreter::Fake(body) => {
            let fake_path = scratch.join("fake_interpreter.sh");
            tokio::fs::write(&fake_path, body).await?;
            let mut perms = tokio::fs::metadata(&fake_path).await?.permissions();
            use std::os::unix::fs::PermissionsExt;
            perms.set_mode(0o755);
            tokio::fs::set_permissions(&fake_path, perms).await?;
            (fake_path, Vec::new(), isolation, system_python_dirs())
        }
        #[cfg(any(test, feature = "test-support"))]
        SelftestInterpreter::MissingBinarySpawnFailure => (
            PathBuf::from("/nonexistent/mcphost-sandboxready-missing-binary"),
            Vec::new(),
            IsolationMechanism::None,
            Vec::new(),
        ),
    };

    let spec = sandbox::RunSpec {
        interpreter: bin,
        interpreter_args: args,
        script_path,
        scratch_dir: scratch.clone(),
        read_only_dirs,
        stdin_payload: source.as_bytes().to_vec(),
        limits: ResourceLimits {
            cpu_seconds: 5,
            memory_mb: 128,
            max_open_files: 32,
            max_file_size_mb: 4,
        },
        wall_clock_timeout: Duration::from_secs(7),
        network: NetworkMode::None,
        extra_env: vec![],
        isolation,
    };
    Ok((scratch, spec))
}

/// PRD-mcphost-python-kind-runtime requirement 1/AC5: CPython's own
/// `SyntaxError.msg` already *names* several restricted constructs
/// precisely (e.g. `"cannot use assignment expressions with subscript"`
/// for `d[k := v]`, since Python's grammar only allows a plain name as an
/// assignment-expression target -- see `PRD-mcphost-python-kind-runtime`'s
/// AC6 analysis in `docs/kinds/python.md`) but never says what to do
/// instead. This appends the accepted alternative to the messages the
/// AST-check rejection path is known to see, so the whole rejection is one
/// self-contained sentence naming both the offending construct and the
/// fix. A message this table doesn't recognize passes through unchanged --
/// CPython's raw `msg` is already the best information mcphost has for an
/// error shape nobody has named an alternative for yet.
fn enhance_syntax_message(message: &str) -> String {
    const ASSIGNMENT_EXPR_ALTERNATIVES: &[(&str, &str)] = &[
        (
            "cannot use assignment expressions with subscript",
            "assign to a plain name first, then set the subscript in a separate statement (e.g. `tmp = value; d[key] = tmp`)",
        ),
        (
            "cannot use assignment expressions with attribute",
            "assign to a plain name first, then set the attribute in a separate statement (e.g. `tmp = value; obj.attr = tmp`)",
        ),
    ];
    for (needle, alternative) in ASSIGNMENT_EXPR_ALTERNATIVES {
        if message.contains(needle) {
            return format!("{message} -- {alternative}");
        }
    }
    if message.contains("assignment expression") {
        // Any other assignment-expression restriction CPython reports
        // that isn't in the table above still gets a generic, honest
        // fallback naming the construct and a safe general alternative,
        // rather than leaving the sentence without one.
        return format!(
            "{message} -- use a separate assignment statement instead (`:=` may only target a plain name)"
        );
    }
    message.to_string()
}

async fn ast_check(source: &str, isolation: IsolationMechanism) -> Result<(), KindError> {
    let (scratch, spec) =
        build_ast_check_spec(source, isolation, &SelftestInterpreter::RealPython3)
            .await
            .map_err(|e| KindError::Exec(format!("ast-check scratch dir: {e}")))?;
    let outcome = sandbox::run(spec).await;
    let _ = tokio::fs::remove_dir_all(&scratch).await;
    let outcome = outcome.map_err(|e| KindError::Exec(format!("ast-check spawn: {e}")))?;

    let SandboxOutcome::Exited { stdout, .. } = outcome else {
        return Err(KindError::Exec(format!(
            "ast-check did not complete cleanly: {outcome:?}"
        )));
    };
    let envelope: Value = serde_json::from_slice(&stdout)
        .map_err(|e| KindError::Exec(format!("ast-check produced non-JSON output: {e}")))?;
    if envelope.get("ok").and_then(Value::as_bool) == Some(true) {
        return Ok(());
    }
    let message = envelope
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("source is not a valid python tool");
    let message = enhance_syntax_message(message);
    // PRD-mcphost-tool-test AC13: a publish-time validation failure carries
    // the same structured detail fields `host.spec_test`'s own failure
    // response does (`describe_test_failure`'s `exception_class`) -- here
    // taken straight from `AST_CHECK_SCRIPT`'s own `error` field
    // ("SyntaxError" or "NoMain") rather than re-derived, and reused by
    // both `host.tool_publish` and `host.spec_test` since both funnel
    // through this same `validate_async` -> `ast_check` call. Kept as
    // `KindError::structured_with("invalid_spec", ..)` rather than a new
    // code so the wire `error_code` -- and every existing assertion on
    // `err.message` -- is unchanged; only `data` gains fields.
    let exception_class = envelope
        .get("error")
        .and_then(Value::as_str)
        .unwrap_or("InvalidSpec");
    match envelope.get("line").and_then(Value::as_i64) {
        Some(line) => Err(KindError::structured_with(
            "invalid_spec",
            format!("source: syntax error at line {line}: {message}"),
            json!({"field": "source", "exception_class": exception_class, "line": line}),
        )),
        None => Err(KindError::structured_with(
            "invalid_spec",
            format!("source: {message}"),
            json!({"field": "source", "exception_class": exception_class}),
        )),
    }
}

fn system_python_dirs() -> Vec<PathBuf> {
    ["/usr", "/lib", "/lib64", "/bin"]
        .into_iter()
        .map(PathBuf::from)
        .filter(|p| p.exists())
        .collect()
}

// ---- sandbox self-test (PRD-mcphost-sandbox-ready) -------------------------
//
// The host's own record of whether its sandbox actually works, proven by
// running one sandboxed process (the same RunSpec-building path `ast_check`
// uses -- see `build_ast_check_spec`) at start and on a schedule, rather
// than trusting `detect_mechanism`'s "the binary is on $PATH" check
// (module docs: this PRD's whole premise is that check alone is not
// enough). Owns its own status cell and knows how to evict the warm pool on
// a ready->unready flip so a warm sandbox spawned while healthy can't keep
// answering calls once the host is known broken (requirement 4).

/// Default re-probe interval while the sandbox is unready
/// (`$MCPHOST_SANDBOX_RECHECK_SECS`, requirement 4).
const DEFAULT_RECHECK_UNREADY_SECS: u64 = 300;
/// Default re-probe interval while the sandbox is ready (requirement 4;
/// not itself configurable in production -- see `SandboxSelftest::interval`
/// for the one exception this crate's own tests take).
const DEFAULT_RECHECK_READY_SECS: u64 = 3600;

struct SandboxSelftest {
    status: RwLock<sandbox::SandboxStatus>,
    isolation: IsolationMechanism,
    /// Test-only override of the interpreter the probe execs (production
    /// always uses `SelftestInterpreter::RealPython3`); mutable so a test
    /// can simulate the sandbox breaking or recovering mid-run (AC5/AC6)
    /// without tearing down and re-registering the whole `Kind`.
    interpreter: RwLock<SelftestInterpreter>,
    recheck_unready_secs: u64,
    /// `None` in production (fixed 3600s per requirement 4); `Some(secs)`
    /// only from a test constructor, which needs the *ready*-state interval
    /// configurable too so AC6 ("given sandbox_ready: true ... when the
    /// periodic recheck runs [with a 1s interval] ... within 3s") doesn't
    /// need a real hour-long wait to observe a ready->unready flip.
    recheck_ready_secs_override: Option<u64>,
    warm: Arc<WarmPool>,
}

impl SandboxSelftest {
    fn new(isolation: IsolationMechanism, warm: Arc<WarmPool>) -> Arc<Self> {
        let recheck_unready_secs = std::env::var("MCPHOST_SANDBOX_RECHECK_SECS")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(DEFAULT_RECHECK_UNREADY_SECS);
        Arc::new(Self {
            // A placeholder, never observable in production: `main.rs`
            // awaits `PythonKind::run_startup_selftest` to completion
            // before the HTTP listener ever starts accepting connections,
            // so no real request can see this default. Tests that never
            // call `run_startup_selftest` (the overwhelming majority --
            // every pre-existing `python_ac*`/`infer_ac*`/`http_ac*` test)
            // see `ready: true` forever, which is what they need: those
            // tests publish and call real python tools and must not be
            // gated by a self-test they never asked for or ran.
            status: RwLock::new(sandbox::SandboxStatus {
                ready: true,
                mechanism: isolation,
                detail: format!("{}: not yet checked", isolation.as_str()),
                checked_at: crate::state::rfc3339_now(),
            }),
            isolation,
            interpreter: RwLock::new(SelftestInterpreter::RealPython3),
            recheck_unready_secs,
            recheck_ready_secs_override: None,
            warm,
        })
    }

    fn read_status(&self) -> sandbox::SandboxStatus {
        self.status
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// Test-only: this method's only callers
    /// (`set_selftest_interpreter_for_test`,
    /// `set_selftest_missing_binary_for_test`) are themselves gated behind
    /// `cfg(test)`/`test-support`, so a default-features build never calls
    /// it -- gate it the same way rather than leaving it dead.
    ///
    /// PRD-mcphost-classify-precision.
    #[cfg(any(test, feature = "test-support"))]
    fn set_interpreter(&self, interpreter: SelftestInterpreter) {
        *self.interpreter.write().unwrap_or_else(|e| e.into_inner()) = interpreter;
    }

    /// Requirement 1: runs one sandboxed process through the exact
    /// RunSpec-building path `ast_check` uses, on the trivial source
    /// `def main(args):\n    return {}\n`. Never returns an `Err` of its
    /// own -- a probe that can't even build its `RunSpec` (e.g. the scratch
    /// dir can't be created) is itself a `binary_missing`-classified
    /// "not ready" status, not a crate-level failure.
    async fn probe(&self) -> sandbox::SandboxStatus {
        const TRIVIAL_SOURCE: &str = "def main(args):\n    return {}\n";
        let interpreter = self
            .interpreter
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        let built = build_ast_check_spec(TRIVIAL_SOURCE, self.isolation, &interpreter).await;
        let (scratch, spec) = match built {
            Ok(v) => v,
            Err(e) => {
                return sandbox::SandboxStatus::from_probe(self.isolation, Err(e));
            }
        };
        let outcome = sandbox::run(spec).await;
        let _ = tokio::fs::remove_dir_all(&scratch).await;
        sandbox::SandboxStatus::from_probe(self.isolation, outcome)
    }

    /// Requirement 5: the one structured log line at start.
    fn log_startup(status: &sandbox::SandboxStatus) {
        if status.ready {
            tracing::info!(
                mechanism = status.mechanism.as_str(),
                ready = status.ready,
                detail = %status.detail,
                "sandbox self-test at start"
            );
        } else {
            tracing::error!(
                mechanism = status.mechanism.as_str(),
                ready = status.ready,
                detail = %status.detail,
                "sandbox self-test at start"
            );
        }
    }

    /// Requirement 4: a transition is logged at `warn` (ready -> unready) or
    /// `info` (unready -> ready); no line at all when the state didn't
    /// change (a periodic recheck that finds the same answer is not a
    /// transition).
    fn log_transition(was_ready: bool, status: &sandbox::SandboxStatus) {
        if was_ready == status.ready {
            return;
        }
        if status.ready {
            tracing::info!(
                mechanism = status.mechanism.as_str(),
                detail = %status.detail,
                "sandbox self-test transitioned unready -> ready"
            );
        } else {
            tracing::warn!(
                mechanism = status.mechanism.as_str(),
                detail = %status.detail,
                "sandbox self-test transitioned ready -> unready"
            );
        }
    }

    /// Requirement 1/5: the startup probe. Awaited by `main.rs` before the
    /// HTTP listener starts accepting, then spawns the periodic re-probe
    /// task (requirement 4) -- called exactly once per process.
    async fn startup(self: &Arc<Self>) -> sandbox::SandboxStatus {
        let status = self.probe().await;
        *self.status.write().unwrap_or_else(|e| e.into_inner()) = status.clone();
        Self::log_startup(&status);
        self.spawn_recheck_loop();
        status
    }

    /// Requirement 4: re-runs the probe, updates the recorded status,
    /// evicts the entire warm pool on a ready->unready flip, and logs the
    /// transition. Shared by the periodic background task and
    /// `admin.sandbox_recheck`'s on-demand path.
    async fn recheck(&self) -> sandbox::SandboxStatus {
        let was_ready = self.read_status().ready;
        let status = self.probe().await;
        *self.status.write().unwrap_or_else(|e| e.into_inner()) = status.clone();
        if was_ready && !status.ready {
            self.warm.evict_all().await;
        }
        Self::log_transition(was_ready, &status);
        status
    }

    /// The interval to sleep before the next periodic recheck: an explicit
    /// per-instance override (test-only, see `recheck_ready_secs_override`)
    /// always wins regardless of current readiness -- this is how AC6's
    /// "recheck runs on a 1s interval while ready" is exercised without
    /// mutating process-wide environment (this crate's own tests avoid
    /// that; see `state.rs`'s `signup_rate_limit_*` tests for the same
    /// stated reason). With no override, production's own defaults apply:
    /// `MCPHOST_SANDBOX_RECHECK_SECS` (default 300) while unready, a fixed
    /// 3600s while ready (requirement 4).
    fn interval(&self, ready: bool) -> Duration {
        if let Some(secs) = self.recheck_ready_secs_override {
            return Duration::from_secs(secs);
        }
        if ready {
            Duration::from_secs(DEFAULT_RECHECK_READY_SECS)
        } else {
            Duration::from_secs(self.recheck_unready_secs)
        }
    }

    fn spawn_recheck_loop(self: &Arc<Self>) {
        // Mirrors `spawn_warm_reaper`'s own guard: a `SandboxSelftest`
        // constructed from plain sync code with no Tokio runtime active
        // (this crate's few sync unit tests that build a `PythonKind` just
        // to call a synchronous method) must not panic trying to spawn.
        if tokio::runtime::Handle::try_current().is_err() {
            return;
        }
        let this = self.clone();
        tokio::spawn(async move {
            loop {
                let ready = this.read_status().ready;
                tokio::time::sleep(this.interval(ready)).await;
                this.recheck().await;
            }
        });
    }
}

// ---- environment build & cache (requirement 3) -----------------------------

#[derive(Debug, Clone)]
enum EnvStatus {
    Building,
    Ready {
        python: PathBuf,
        site_packages: String,
    },
    Failed {
        tail: String,
    },
}

fn requirements_hash(reqs: &[String]) -> String {
    let mut sorted = reqs.to_vec();
    sorted.sort();
    let mut hasher = Sha256::new();
    hasher.update(sorted.join("\n").as_bytes());
    let digest = hasher.finalize();
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

fn ready_marker(env_dir: &Path) -> PathBuf {
    env_dir.join(".ready")
}
fn failed_marker(env_dir: &Path) -> PathBuf {
    env_dir.join(".failed")
}

/// True if `bin` resolves via the process's current `$PATH` -- the same
/// lookup `std::process::Command` itself does when given a bare program
/// name, duplicated here so [`ensure_uv_discoverable_for_test`] can check
/// before mutating anything.
fn resolves_on_path(bin: &str) -> bool {
    std::env::var_os("PATH")
        .map(|path| std::env::split_paths(&path).any(|dir| dir.join(bin).is_file()))
        .unwrap_or(false)
}

/// Test-only fixture: makes `uv` discoverable via `$PATH` for the rest of
/// this process, if it is not already.
///
/// `run_build_steps` below spawns `uv` (bare name, PATH-resolved) to build
/// each test's real warm-pool sandbox environment -- there is no test
/// double for it, by design (these are AC1-5 correctness tests for the
/// warm pool itself, not for `uv`). `uv`'s official installer places the
/// binary in `~/.local/bin`, which only a *login* shell's rc file adds to
/// `$PATH`; a bare non-login invocation of `cargo test` (e.g. over
/// `ssh host cargo test`, or from a harness that execs cargo directly)
/// inherits a `$PATH` without it, so the very first cold call in any of
/// these tests fails to spawn `uv` with `ENOENT` -- deterministically,
/// regardless of test order, since nothing else in this process ever
/// changes `$PATH` on its behalf. Confirmed by reproducing the reported
/// failure with `env -i PATH=<default, no ~/.local/bin> cargo test`.
///
/// This is additive-only (it only prepends a directory to the existing
/// `$PATH`, never removes or overrides anything already resolvable) and
/// runs at most once per process via [`std::sync::Once`], called from the
/// very first line of [`PythonKind::for_test_with_warm_pool`] -- i.e.
/// before any of these tests has had a chance to reach an actual `spawn`
/// several `await`s later (semaphore/build-queue acquire, then
/// `tokio::fs::create_dir_all`), so the mutation is settled well before
/// any concurrently-running sibling test could be spawning a process that
/// reads `$PATH`.
fn ensure_uv_discoverable_for_test() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        if resolves_on_path("uv") {
            return;
        }
        let Some(home) = std::env::var_os("HOME") else {
            return;
        };
        let candidates = [
            PathBuf::from(&home).join(".local/bin"),
            PathBuf::from(&home).join(".cargo/bin"),
        ];
        let Some(uv_dir) = candidates.into_iter().find(|dir| dir.join("uv").is_file()) else {
            return;
        };
        let existing = std::env::var_os("PATH").unwrap_or_default();
        let mut dirs: Vec<PathBuf> = vec![uv_dir];
        dirs.extend(std::env::split_paths(&existing));
        if let Ok(joined) = std::env::join_paths(dirs) {
            // `Once` guarantees this runs at most once, and it only ever
            // prepends a directory to `$PATH` -- see the doc comment above
            // for why no other test-owned code can be racing a read of
            // `$PATH` at this point in a test's lifetime.
            // SAFETY: single-shot, prepend-only mutation with no concurrent readers.
            unsafe {
                std::env::set_var("PATH", joined);
            }
        }
    });
}

/// Runs `uv venv` + (if any) `uv pip install`, all under the PRD's 120s
/// build timeout, and writes the durable disk marker so a process restart
/// doesn't rebuild an already-good environment.
async fn build_env(env_dir: PathBuf, requirements: Vec<String>) -> EnvStatus {
    let result =
        tokio::time::timeout(BUILD_TIMEOUT, run_build_steps(&env_dir, &requirements)).await;
    match result {
        Ok(Ok(site_packages)) => {
            let _ = tokio::fs::write(ready_marker(&env_dir), &site_packages).await;
            EnvStatus::Ready {
                python: env_dir.join("bin").join("python"),
                site_packages,
            }
        }
        Ok(Err(tail)) => {
            let _ = tokio::fs::write(failed_marker(&env_dir), &tail).await;
            EnvStatus::Failed { tail }
        }
        Err(_elapsed) => {
            let tail = "build timed out after 120s".to_string();
            let _ = tokio::fs::write(failed_marker(&env_dir), &tail).await;
            EnvStatus::Failed { tail }
        }
    }
}

async fn run_command_tail(mut cmd: tokio::process::Command) -> Result<Vec<u8>, String> {
    let output = cmd
        .output()
        .await
        .map_err(|e| format!("spawn failed: {e}"))?;
    if output.status.success() {
        Ok(output.stdout)
    } else {
        let mut tail = output.stdout;
        tail.extend_from_slice(&output.stderr);
        let start = tail.len().saturating_sub(4096);
        Err(String::from_utf8_lossy(&tail[start..]).into_owned())
    }
}

async fn run_build_steps(env_dir: &Path, requirements: &[String]) -> Result<String, String> {
    tokio::fs::create_dir_all(env_dir)
        .await
        .map_err(|e| format!("mkdir env dir: {e}"))?;

    let mut venv_cmd = tokio::process::Command::new("uv");
    venv_cmd.arg("venv").arg(env_dir);
    run_command_tail(venv_cmd).await?;

    let python_bin = env_dir.join("bin").join("python");
    if !requirements.is_empty() {
        let mut install_cmd = tokio::process::Command::new("uv");
        install_cmd
            .arg("pip")
            .arg("install")
            .arg("--python")
            .arg(&python_bin)
            .args(requirements);
        run_command_tail(install_cmd).await?;
    }

    let mut sitepkg_cmd = tokio::process::Command::new(&python_bin);
    sitepkg_cmd.args([
        "-c",
        "import sysconfig; print(sysconfig.get_paths()['purelib'], end='')",
    ]);
    let raw = run_command_tail(sitepkg_cmd).await?;
    Ok(String::from_utf8_lossy(&raw).into_owned())
}

/// In-memory cache of every environment this process has seen, backed by
/// the on-disk `.ready`/`.failed` markers for cross-restart durability.
struct EnvRegistry {
    states: Mutex<HashMap<PathBuf, EnvStatus>>,
    /// Requirement 3: "a per-tenant build queue of 1" -- one semaphore per
    /// namespace, created on first use.
    build_queues: Mutex<HashMap<String, Arc<Semaphore>>>,
}

impl EnvRegistry {
    fn new() -> Self {
        Self {
            states: Mutex::new(HashMap::new()),
            build_queues: Mutex::new(HashMap::new()),
        }
    }

    fn queue_for(&self, namespace: &str) -> Arc<Semaphore> {
        let mut guard = self.build_queues.lock().unwrap_or_else(|e| e.into_inner());
        guard
            .entry(namespace.to_string())
            .or_insert_with(|| Arc::new(Semaphore::new(1)))
            .clone()
    }

    /// Reads back a status already known in-memory, or (on a cold cache)
    /// whatever the filesystem markers say. Never triggers a build itself.
    async fn status(&self, env_dir: &Path) -> Option<EnvStatus> {
        if let Some(status) = self
            .states
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(env_dir)
            .cloned()
        {
            return Some(status);
        }
        if let Ok(site_packages) = tokio::fs::read_to_string(ready_marker(env_dir)).await {
            let status = EnvStatus::Ready {
                python: env_dir.join("bin").join("python"),
                site_packages,
            };
            self.set(env_dir, status.clone());
            return Some(status);
        }
        if let Ok(tail) = tokio::fs::read_to_string(failed_marker(env_dir)).await {
            let status = EnvStatus::Failed { tail };
            self.set(env_dir, status.clone());
            return Some(status);
        }
        None
    }

    fn set(&self, env_dir: &Path, status: EnvStatus) {
        self.states
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(env_dir.to_path_buf(), status);
    }

    /// Marks `env_dir` as `Building` and spawns the real build in the
    /// background, gated on this namespace's build-queue-of-1 semaphore.
    /// Never awaited by the caller -- requirement 3: a call during the
    /// build returns `tool_building` immediately.
    fn start_build(
        self: &Arc<Self>,
        env_dir: PathBuf,
        namespace: String,
        requirements: Vec<String>,
    ) {
        self.set(&env_dir, EnvStatus::Building);
        let this = self.clone();
        tokio::spawn(async move {
            let permit = this.queue_for(&namespace).acquire_owned().await;
            let status = build_env(env_dir.clone(), requirements).await;
            drop(permit);
            this.set(&env_dir, status);
        });
    }
}

// ---- per-tenant CPU budget (requirement 8, AC14) ---------------------------

struct CpuWindow {
    started: Instant,
    used_ms: u64,
}

struct CpuBudget {
    budget_ms: u64,
    windows: Mutex<HashMap<i64, CpuWindow>>,
}

impl CpuBudget {
    fn new(budget_ms: u64) -> Self {
        Self {
            budget_ms,
            windows: Mutex::new(HashMap::new()),
        }
    }

    /// `Err(retry_after_s)` if `tenant_id` is already over budget for the
    /// current hour window.
    fn check(&self, tenant_id: i64) -> Result<(), u64> {
        let now = Instant::now();
        let mut guard = self.windows.lock().unwrap_or_else(|e| e.into_inner());
        let window = guard.entry(tenant_id).or_insert_with(|| CpuWindow {
            started: now,
            used_ms: 0,
        });
        if now.duration_since(window.started) >= CPU_BUDGET_WINDOW {
            window.started = now;
            window.used_ms = 0;
        }
        if window.used_ms >= self.budget_ms {
            let remaining = CPU_BUDGET_WINDOW.saturating_sub(now.duration_since(window.started));
            return Err(remaining.as_secs().max(1));
        }
        Ok(())
    }

    fn record(&self, tenant_id: i64, cpu_ms: i64) {
        let now = Instant::now();
        let mut guard = self.windows.lock().unwrap_or_else(|e| e.into_inner());
        let window = guard.entry(tenant_id).or_insert_with(|| CpuWindow {
            started: now,
            used_ms: 0,
        });
        window.used_ms = window.used_ms.saturating_add(cpu_ms.max(0) as u64);
    }
}

// ---- warm pool (requirement 1, AC1/AC2/AC3/AC4/AC5/AC8) --------------------

/// Identifies one tool for pool purposes: which tenant, and which of its
/// local names. See the module doc's "Pool key" note for why this (rather
/// than a pure content hash) is the map key.
type ToolKey = (i64, String);

/// One idle, ready-to-reuse sandbox. `fingerprint` is `hash(source +
/// sorted(requirements))` -- computed fresh on every call and compared
/// against this before reuse, so a republish that lands under the *same*
/// name (caught synchronously by `on_tool_changed` too, belt and braces)
/// can never hand a stale sandbox back even if the eviction call somehow
/// raced it.
struct WarmEntry {
    sandbox: PersistentSandbox,
    fingerprint: String,
    site_packages: String,
    /// The scratch directory holding this entry's `tool.py`/`runner.py`,
    /// alive for as long as the sandbox itself is -- removed only when the
    /// entry is killed, unlike a cold call's scratch dir (removed right
    /// after that one call).
    scratch_dir: PathBuf,
    last_used: Instant,
}

/// Kills `entry`'s sandbox and removes its scratch directory -- every path
/// that discards a [`WarmEntry`] (decline-to-pool, eviction, reap, an
/// entry replaced by a newer `offer`) goes through this so the two always
/// happen together.
async fn kill_warm_entry(entry: WarmEntry) {
    entry.sandbox.kill().await;
    let _ = tokio::fs::remove_dir_all(&entry.scratch_dir).await;
}

/// Per-tenant and box-wide bounded pool of idle [`PersistentSandbox`]es,
/// reaped on TTL (AC2) and evicted synchronously on republish/remove/tenant
/// removal (AC3). One instance per [`PythonKind`], shared with its
/// background reaper task via `Arc`.
struct WarmPool {
    entries: Mutex<HashMap<ToolKey, WarmEntry>>,
    ttl: Duration,
    per_tenant: usize,
    max_total: usize,
    hits: AtomicU64,
    misses: AtomicU64,
}

impl WarmPool {
    fn new(ttl: Duration, per_tenant: usize, max_total: usize) -> Self {
        Self {
            entries: Mutex::new(HashMap::new()),
            ttl,
            per_tenant,
            max_total,
            hits: AtomicU64::new(0),
            misses: AtomicU64::new(0),
        }
    }

    /// Takes the entry for `key` out of the pool if one exists -- reused
    /// (fingerprint matches) or stale (doesn't, or simply present for a
    /// different requirements/source pairing than this call needs). Either
    /// way the entry is gone from the pool the moment this returns, so two
    /// concurrent calls to the same tool never race for the same sandbox
    /// (AC's "a warm sandbox runs one call at a time" -- enforced here by
    /// construction, not a lock held across the call).
    fn take(&self, key: &ToolKey) -> Option<WarmEntry> {
        self.entries
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(key)
    }

    /// Counts this call's outcome for `host.usage`/`admin.usage` (see the
    /// module doc's scoped-down metrics note: tracked here, not yet piped
    /// out over an RPC).
    fn record_hit(&self) {
        self.hits.fetch_add(1, Ordering::Relaxed);
    }
    fn record_miss(&self) {
        self.misses.fetch_add(1, Ordering::Relaxed);
    }

    fn pool_size(&self) -> usize {
        self.entries.lock().unwrap_or_else(|e| e.into_inner()).len()
    }

    /// `true` if `tenant_id` has room for one more warm entry (its own
    /// per-tenant bound not yet reached, and the box-wide bound not yet
    /// reached either) -- checked before bothering to spawn a candidate
    /// sandbox at all, so a full pool doesn't pay a spawn cost only to
    /// discard the result in [`Self::offer`].
    fn has_room(&self, tenant_id: i64) -> bool {
        let guard = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        if guard.len() >= self.max_total {
            return false;
        }
        guard.keys().filter(|(t, _)| *t == tenant_id).count() < self.per_tenant
    }

    fn metrics(&self) -> (u64, u64, usize) {
        (
            self.hits.load(Ordering::Relaxed),
            self.misses.load(Ordering::Relaxed),
            self.pool_size(),
        )
    }

    /// Inserts a freshly-spawned idle sandbox for `key`, honoring both
    /// bounds (requirement 1: `MCPHOST_PY_WARM_PER_TENANT`,
    /// `MCPHOST_PY_WARM_MAX`) -- if there's no room, `sandbox` is killed
    /// instead of inserted (the caller's own call already completed
    /// successfully using it or a cold path; declining to pool it just
    /// means the next call is cold too, not a failure of any kind).
    async fn offer(&self, key: ToolKey, entry: WarmEntry) {
        let should_insert = {
            let guard = self.entries.lock().unwrap_or_else(|e| e.into_inner());
            if guard.len() >= self.max_total {
                false
            } else {
                let tenant_count = guard.keys().filter(|(t, _)| *t == key.0).count();
                tenant_count < self.per_tenant
            }
        };
        if !should_insert {
            kill_warm_entry(entry).await;
            return;
        }
        let evicted = self
            .entries
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(key, entry);
        if let Some(evicted) = evicted {
            kill_warm_entry(evicted).await;
        }
    }

    /// AC3: kill and remove every warm entry for one tool, synchronously --
    /// called from `Kind::on_tool_changed` before `host.tool_publish`/
    /// `host.tool_remove` returns.
    async fn evict_tool(&self, tenant_id: i64, local_name: &str) {
        let removed = self
            .entries
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&(tenant_id, local_name.to_string()));
        if let Some(entry) = removed {
            kill_warm_entry(entry).await;
        }
    }

    /// AC3: kill and remove every warm entry for one tenant, regardless of
    /// tool name -- called from `Kind::on_tenant_removed`.
    async fn evict_tenant(&self, tenant_id: i64) {
        let victims: Vec<ToolKey> = {
            let guard = self.entries.lock().unwrap_or_else(|e| e.into_inner());
            guard
                .keys()
                .filter(|(t, _)| *t == tenant_id)
                .cloned()
                .collect()
        };
        for key in victims {
            // The `MutexGuard` this `.lock()` produces must not still be
            // alive across the `kill_warm_entry(...).await` below (it isn't
            // `Send`, and Rust's temporary-lifetime-extension rule would
            // otherwise keep it alive for the whole `if let` block) -- so
            // the removal is its own statement, ended before the `if let`.
            let entry = self
                .entries
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(&key);
            if let Some(entry) = entry {
                kill_warm_entry(entry).await;
            }
        }
    }

    /// PRD-mcphost-sandbox-ready requirement 4 (AC6): kill and remove every
    /// warm entry box-wide, regardless of tenant or tool -- called on a
    /// ready->unready sandbox flip, since no existing warm sandbox can be
    /// trusted to keep serving once the host's own self-test says the
    /// mechanism is broken, even one whose process happens to still be
    /// alive.
    async fn evict_all(&self) {
        let victims: Vec<ToolKey> = {
            let guard = self.entries.lock().unwrap_or_else(|e| e.into_inner());
            guard.keys().cloned().collect()
        };
        for key in victims {
            // See the same note in `evict_tenant` below.
            let entry = self
                .entries
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(&key);
            if let Some(entry) = entry {
                kill_warm_entry(entry).await;
            }
        }
    }

    /// AC2: kill and remove every entry idle past `self.ttl`. Run
    /// periodically by [`PythonKind::spawn_reaper`].
    async fn reap_expired(&self) {
        let now = Instant::now();
        let expired: Vec<ToolKey> = {
            let guard = self.entries.lock().unwrap_or_else(|e| e.into_inner());
            guard
                .iter()
                .filter(|(_, e)| now.duration_since(e.last_used) >= self.ttl)
                .map(|(k, _)| k.clone())
                .collect()
        };
        for key in expired {
            // See the same note in `evict_tenant` above.
            let entry = self
                .entries
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(&key);
            if let Some(entry) = entry {
                kill_warm_entry(entry).await;
            }
        }
    }
}

/// Spawns the background task that periodically reaps TTL-expired warm
/// entries (AC2). One per [`PythonKind`] instance (each `PythonKind::build`
/// call gets its own `WarmPool` and its own reaper) -- never joined; it runs
/// for the process's lifetime, same as `EnvRegistry`'s own background
/// builds, and is naturally dropped along with the runtime on shutdown.
fn spawn_warm_reaper(warm: Arc<WarmPool>) {
    // `PythonKind::new`/`build` runs from ordinary sync code in a couple of
    // this crate's own unit tests (constructing a kind purely to call its
    // synchronous `validate`, never touching the warm pool) with no Tokio
    // runtime active -- `tokio::spawn` would panic there. A pool that's
    // never used doesn't need a reaper; every real caller (`main.rs`,
    // every `#[tokio::test]`) *is* inside a runtime, so this is the only
    // context where skipping the spawn is reachable.
    if tokio::runtime::Handle::try_current().is_err() {
        return;
    }
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(WARM_REAP_INTERVAL).await;
            warm.reap_expired().await;
        }
    });
}

/// `hash(source + sorted(requirements) + sorted(secret env))` -- the warm
/// pool's own reuse key (module doc: "Pool key"), distinct from
/// [`requirements_hash`] (which keys the *environment*/venv cache and
/// deliberately ignores `source`, since many tools can share one venv).
/// Secret values are folded in too: a warm sandbox bakes its `SECRET_*`
/// environment variables in at spawn time (a persistent process can't be
/// handed new env vars mid-life the way a cold call's fresh subprocess
/// gets them), so a tenant rotating a secret via `host.secret_set` between
/// calls must be a fingerprint miss, not a reuse of a sandbox holding the
/// old value.
fn call_fingerprint(
    source: &str,
    requirements: &[String],
    secret_env: &[(String, String)],
) -> String {
    let mut sorted = requirements.to_vec();
    sorted.sort();
    let mut sorted_secrets = secret_env.to_vec();
    sorted_secrets.sort();
    let mut hasher = Sha256::new();
    hasher.update(source.as_bytes());
    hasher.update(b"\n--reqs--\n");
    hasher.update(sorted.join("\n").as_bytes());
    hasher.update(b"\n--secrets--\n");
    for (k, v) in &sorted_secrets {
        hasher.update(k.as_bytes());
        hasher.update(b"=");
        hasher.update(v.as_bytes());
        hasher.update(b"\n");
    }
    let digest = hasher.finalize();
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

// ---- runner protocol envelope (this kind's own convention) ---------------
//
// PRD-mcphost-code-tools-warm-pool requirement 1/5: this script is now a
// *loop* -- one JSON request per line in on stdin, one JSON response per
// line out on stdout -- so the exact same script serves both a cold call
// (the caller writes one line, then closes stdin; the `for` loop below ends
// naturally at EOF after processing it) and a warm sandbox kept alive across
// many calls (`sandbox::PersistentSandbox`, whose caller keeps stdin open
// and sends one more line per call). Requirement 5 / AC4's "fresh module
// namespace each call": every iteration re-imports `tool.py` via
// `importlib.util.spec_from_file_location` into a brand new module object,
// exactly as the pre-warm-pool one-shot script always did -- the only thing
// that changed is that this now happens in a loop instead of once, so a
// module-level global set in one call is never visible to the next
// regardless of whether the sandbox serving it is warm or cold.
//
// Every response line is a single clean JSON object -- nothing the tool
// itself prints or writes to `sys.stderr` is allowed to reach the real fd 1,
// which would otherwise corrupt this line-based protocol the moment a tool
// calls `print(...)`. `sys.stdout`/`sys.stderr` are redirected to in-memory
// buffers for the duration of `main(args)` and folded into the envelope as
// `stdout_capture`/`stderr_capture` instead -- `host.tool_run` (this PRD's
// other half) reads those two fields for its "full stdout and stderr"
// response; an ordinary call simply ignores them.
const PY_RUNNER_SCRIPT: &str = r#"
import sys, json, importlib.util, traceback, io

_real_stdout = sys.stdout

def emit(obj):
    _real_stdout.write(json.dumps(obj))
    _real_stdout.write("\n")
    _real_stdout.flush()

def run_one(payload):
    site_packages = payload.get("site_packages")
    if site_packages and site_packages not in sys.path:
        sys.path.insert(0, site_packages)
    args = payload.get("args", {})

    out_buf, err_buf = io.StringIO(), io.StringIO()
    old_out, old_err = sys.stdout, sys.stderr
    sys.stdout, sys.stderr = out_buf, err_buf
    try:
        try:
            spec = importlib.util.spec_from_file_location("tool", "tool.py")
            module = importlib.util.module_from_spec(spec)
            spec.loader.exec_module(module)
            if not hasattr(module, "main"):
                obj = {"ok": False, "kind": "exception", "error": "AttributeError",
                       "message": "tool module has no main(args)", "traceback": ""}
            else:
                try:
                    result = module.main(args)
                except MemoryError:
                    obj = {"ok": False, "kind": "oom"}
                except Exception as e:
                    obj = {"ok": False, "kind": "exception", "error": type(e).__name__,
                           "message": str(e), "traceback": traceback.format_exc()}
                else:
                    try:
                        json.dumps(result)
                    except (TypeError, ValueError) as e:
                        obj = {"ok": False, "kind": "output_invalid", "message": str(e)}
                    else:
                        obj = {"ok": True, "result": result}
        except MemoryError:
            obj = {"ok": False, "kind": "oom"}
    finally:
        sys.stdout, sys.stderr = old_out, old_err
    obj["stdout_capture"] = out_buf.getvalue()[-200000:]
    obj["stderr_capture"] = err_buf.getvalue()[-200000:]
    return obj

for line in sys.stdin:
    line = line.strip()
    if not line:
        continue
    try:
        payload = json.loads(line)
    except Exception as e:
        emit({"ok": False, "kind": "exception", "error": "PayloadError", "message": str(e),
              "stdout_capture": "", "stderr_capture": ""})
        continue
    emit(run_one(payload))
"#;

/// PRD-mcphost-python-kind-runtime, requirement 1/AC2: pulls the single
/// most-specific `tool.py` frame (the LAST one -- the innermost, closest to
/// where the exception was actually raised, since `main`'s own body may
/// call further into helper functions also defined in `tool.py`) out of a
/// CPython `traceback.format_exc()` dump. Traceback frames from `runner.py`
/// or Python's own `importlib` machinery are never `tool.py` frames, so
/// this can't accidentally attribute the failure to host code. Returns the
/// 1-based line number and (when the frame carries a source excerpt, which
/// CPython includes whenever the file is readable from disk -- true here,
/// since `tool.py` still exists in the scratch dir at exception time) the
/// stripped source text of that line.
fn extract_tool_source_line(traceback: &str) -> (Option<i64>, Option<String>) {
    let mut found: (Option<i64>, Option<String>) = (None, None);
    let mut lines = traceback.lines().peekable();
    while let Some(line) = lines.next() {
        let Some(after_file) = line.trim_start().strip_prefix("File \"") else {
            continue;
        };
        // CPython always renders the frame's path exactly as it was
        // passed to `compile`/`exec_module` -- `run_one`'s
        // `spec_from_file_location("tool", "tool.py")` uses a bare
        // relative name relative to the runner's cwd (the scratch dir),
        // but the sandbox wrapper may report an absolute path instead
        // (observed: `/tmp/.../scratch/tool.py`) -- match on the
        // filename, not the whole path, so either form is recognized.
        let Some(quote_end) = after_file.find('"') else {
            continue;
        };
        let path = &after_file[..quote_end];
        if !(path == "tool.py" || path.ends_with("/tool.py")) {
            continue;
        }
        let Some(after_line) = after_file[quote_end + 1..].strip_prefix(", line ") else {
            continue;
        };
        let comma = after_line.find(',').unwrap_or(after_line.len());
        let Ok(line_no) = after_line[..comma].trim().parse::<i64>() else {
            continue;
        };
        let source_line = lines
            .peek()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty() && !s.starts_with("File \""));
        found = (Some(line_no), source_line);
    }
    found
}

fn map_envelope_error(envelope: &Value, stderr_tail: &str) -> KindError {
    let kind = envelope.get("kind").and_then(Value::as_str).unwrap_or("");
    match kind {
        "oom" => KindError::structured("tool_oom", "the call exceeded its memory limit"),
        "output_invalid" => KindError::structured_with(
            "tool_output_invalid",
            envelope
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("tool result is not JSON-serializable")
                .to_string(),
            json!({"stderr_tail": stderr_tail}),
        ),
        _ => {
            let error = envelope
                .get("error")
                .and_then(Value::as_str)
                .unwrap_or("Exception");
            let message = envelope
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("");
            let traceback = envelope
                .get("traceback")
                .and_then(Value::as_str)
                .unwrap_or("");
            // Requirement 1/AC2: every exception that escapes the tool's
            // own code is `phase: tool_code`, carries the exception class
            // separately from the message (`error`), and -- when
            // attributable -- the tool-source line the failure actually
            // happened at, not just the full traceback blob (still kept in
            // `data.traceback` for anyone who wants it, but the message
            // itself is a clean one-liner, never a bare traceback
            // fragment).
            let (tool_line, source_line) = extract_tool_source_line(traceback);
            let located = tool_line
                .map(|n| format!(" at tool.py:{n}"))
                .unwrap_or_default();
            let mut data = json!({
                "phase": "tool_code",
                "exception_class": error,
                "traceback": traceback,
                "stderr_tail": stderr_tail,
            });
            if let Some(obj) = data.as_object_mut() {
                if let Some(n) = tool_line {
                    obj.insert("line".to_string(), json!(n));
                }
                if let Some(src) = source_line {
                    obj.insert("source_line".to_string(), json!(src));
                }
            }
            KindError::structured_with("tool_exception", format!("{error}: {message}{located}"), data)
        }
    }
}

/// Parses one runner-protocol response line (whether it came from a cold
/// call's whole stdout or one line read from a [`PersistentSandbox`]) into
/// either the tool's result or the mapped [`KindError`]. Shared by
/// [`map_sandbox_outcome`]'s `Exited` arm and the warm-reuse path in
/// [`PythonKind::call`], which never produces a [`SandboxOutcome`] at all
/// (there's no process exit to classify -- the sandbox is still running).
fn map_envelope_line(line: &[u8]) -> Result<Value, KindError> {
    match serde_json::from_slice::<Value>(line) {
        Ok(envelope) if envelope.get("ok").and_then(Value::as_bool) == Some(true) => {
            Ok(envelope.get("result").cloned().unwrap_or(Value::Null))
        }
        Ok(envelope) => Err(map_envelope_error(&envelope, "")),
        Err(_) => Err(KindError::structured(
            "tool_output_invalid",
            "tool stdout was not valid JSON",
        )),
    }
}

/// The JSON line a caller (cold `sandbox::run`, or a warm
/// `PersistentSandbox::call`) writes to the runner's stdin: the call's
/// arguments plus the venv's `site_packages` path to add to `sys.path`.
fn call_payload(args: &Value, site_packages: &str) -> Vec<u8> {
    serde_json::to_vec(&json!({"args": args, "site_packages": site_packages})).unwrap_or_default()
}

/// Byte-caps `s` to its last `cap` bytes (requirement 3: "capped at 64 KiB
/// each"), landing on a UTF-8 char boundary so the result is always valid
/// `str` (never splits a multi-byte character). `pub(crate)` (PRD-mcphost-tool-test):
/// `kinds::describe_test_failure` reuses this exact tail-capping for a
/// `host.spec_test` exception traceback (bounded to 8 KiB there) rather
/// than a second, slightly-different truncation helper.
pub(crate) fn cap_str_bytes(s: &str, cap: usize) -> String {
    if s.len() <= cap {
        return s.to_string();
    }
    let start = s.len() - cap;
    let mut idx = start;
    while idx < s.len() && !s.is_char_boundary(idx) {
        idx += 1;
    }
    s[idx..].to_string()
}

/// Builds `host.tool_run`'s `{stdout, stderr, exit_code, result}` response
/// (requirement 3, AC6) from a completed cold sandbox run. `exit_code` is
/// synthesized from the envelope's own `ok` flag (0 success, 1 tool-level
/// failure), not the OS exit status of the runner process -- `PY_RUNNER_SCRIPT`'s
/// request loop always exits 0 by design (see that constant's docs)
/// regardless of whether the tool itself raised, so the OS exit status
/// alone can't distinguish the two the way this RPC's callers expect.
fn tool_run_response(outcome: SandboxOutcome, effective_schema: &Value, test_mode: bool) -> Value {
    let (envelope, fallback_exit_code, sandbox_stderr_tail): (Option<Value>, i64, String) =
        match &outcome {
            SandboxOutcome::Exited { stdout, .. } => {
                (serde_json::from_slice(stdout).ok(), 0, String::new())
            }
            SandboxOutcome::NonZeroExit {
                code,
                stdout_tail,
                stderr_tail,
                ..
            } => (
                serde_json::from_str(stdout_tail).ok(),
                *code as i64,
                stderr_tail.clone(),
            ),
            SandboxOutcome::Signaled {
                signal,
                stderr_tail,
                ..
            } => (None, -(*signal as i64), stderr_tail.clone()),
            SandboxOutcome::TimedOut { .. } => (None, -1, String::new()),
        };

    let Some(envelope) = envelope else {
        // No readable envelope at all: a genuine crash, kernel kill or
        // timeout, not an ordinary tool-level exception (which always
        // produces one) -- best-effort from whatever the sandbox itself
        // captured on real stderr.
        let mut result = json!({
            "stdout": "",
            "stderr": cap_str_bytes(&sandbox_stderr_tail, TOOL_RUN_CAP_BYTES),
            "exit_code": fallback_exit_code,
            "result": Value::Null,
        });
        if test_mode && let Some(obj) = result.as_object_mut() {
            obj.insert("schema".to_string(), effective_schema.clone());
        }
        return result;
    };

    let ok = envelope.get("ok").and_then(Value::as_bool).unwrap_or(false);
    let result = if ok {
        envelope.get("result").cloned().unwrap_or(Value::Null)
    } else {
        Value::Null
    };
    let mut stderr_text = envelope
        .get("stderr_capture")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    if !ok {
        let kind = envelope
            .get("kind")
            .and_then(Value::as_str)
            .unwrap_or("exception");
        let message = envelope
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("");
        let traceback = envelope
            .get("traceback")
            .and_then(Value::as_str)
            .unwrap_or("");
        if !stderr_text.is_empty() {
            stderr_text.push('\n');
        }
        stderr_text.push_str(&format!("[{kind}] {message}"));
        if !traceback.is_empty() {
            stderr_text.push('\n');
            stderr_text.push_str(traceback);
        }
    }
    let stdout_text = envelope
        .get("stdout_capture")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();

    let mut result_obj = json!({
        "stdout": cap_str_bytes(&stdout_text, TOOL_RUN_CAP_BYTES),
        "stderr": cap_str_bytes(&stderr_text, TOOL_RUN_CAP_BYTES),
        "exit_code": if ok { 0 } else { 1 },
        "result": result,
    });
    if test_mode && let Some(obj) = result_obj.as_object_mut() {
        obj.insert("schema".to_string(), effective_schema.clone());
    }
    result_obj
}

fn map_sandbox_outcome(outcome: SandboxOutcome) -> Result<Value, KindError> {
    match outcome {
        SandboxOutcome::Exited { stdout, .. } => map_envelope_line(&stdout),
        SandboxOutcome::NonZeroExit {
            stdout_tail,
            stderr_tail,
            ..
        } => match serde_json::from_str::<Value>(&stdout_tail) {
            Ok(envelope) => Err(map_envelope_error(&envelope, &stderr_tail)),
            Err(_) => Err(KindError::structured_with(
                "tool_exception",
                "the tool process exited with an error and produced no readable envelope",
                json!({"stdout_tail": stdout_tail, "stderr_tail": stderr_tail}),
            )),
        },
        SandboxOutcome::Signaled {
            signal,
            stdout_tail,
            stderr_tail,
            ..
        } => Err(KindError::structured_with(
            "tool_exception",
            format!("the tool process was killed by signal {signal}"),
            json!({"stdout_tail": stdout_tail, "stderr_tail": stderr_tail}),
        )),
        SandboxOutcome::TimedOut { .. } => Err(KindError::structured(
            "tool_timeout",
            "the call exceeded its timeout",
        )),
    }
}

// ---- result envelope contract (PRD-mcphost-result-envelope-contract) ------

/// A `Value`'s JSON type name, for the AC3 scalar-promotion warning message.
fn value_type_name(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

/// Requirement 3: promotes `declared` output fields into `result.payload`
/// for a python tool's own returned value -- the same contract `http`'s
/// `call` applies to its response body (requirement 2), but starting from a
/// value that (unlike `http`'s always-object body) may be a bare
/// scalar/list/string. A no-op (returns `value` unchanged) when `declared`
/// is empty, so a tool with no declared outputs sees no envelope changes at
/// all (Migration/compatibility: additive).
///
/// AC2: an object return gets `payload` alongside its existing top-level
/// fields, mirroring the whole value plus any promoted (nested-one-level)
/// field -- same shape `http`'s `body`+`payload` pair already has.
///
/// AC3: a bare scalar/list has no object keys to search, so the whole
/// value is promoted to the *first* declared field, with a structured
/// `_envelope_warning` naming the scalar promotion rather than silently
/// guessing which declared field the raw value represents.
fn apply_declared_outputs(value: Value, declared: &[String]) -> Value {
    if declared.is_empty() {
        return value;
    }
    match value {
        Value::Object(obj) => {
            let source = Value::Object(obj.clone());
            let mut payload_map = obj.clone();
            super::promote_declared_outputs_any_wrapper(&mut payload_map, &source, declared);
            let mut out = obj;
            out.insert("payload".to_string(), Value::Object(payload_map));
            Value::Object(out)
        }
        other => {
            let first = declared[0].clone();
            let type_name = value_type_name(&other);
            let mut payload_map = Map::new();
            payload_map.insert(first.clone(), other.clone());
            payload_map.insert(
                "_envelope_warning".to_string(),
                json!(format!(
                    "tool returned a bare {type_name} but declares outputs {declared:?}; the \
                     whole value was promoted to '{first}' since there are no object keys to \
                     match the rest against -- return an object with matching keys instead"
                )),
            );
            json!({"value": other, "payload": Value::Object(payload_map)})
        }
    }
}

// ---- secret redaction (requirement 7, AC11) --------------------------------
//
// Same shape as `kinds::http`'s own redaction (that module can't be reused
// directly -- it's private to that file), applied to both a successful
// result (a tool may legitimately echo a secret it was handed) and an
// error's message/traceback/output tails.

fn redact_str(input: &str, secrets: &[String]) -> String {
    let mut ordered: Vec<&String> = secrets.iter().filter(|s| !s.is_empty()).collect();
    ordered.sort_by_key(|s| std::cmp::Reverse(s.len()));
    let mut out = input.to_string();
    for secret in ordered {
        out = out.replace(secret.as_str(), "***");
    }
    out
}

fn redact_value(value: &Value, secrets: &[String]) -> Value {
    if secrets.is_empty() {
        return value.clone();
    }
    match value {
        Value::String(s) => Value::String(redact_str(s, secrets)),
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(k, v)| (k.clone(), redact_value(v, secrets)))
                .collect(),
        ),
        Value::Array(arr) => Value::Array(arr.iter().map(|v| redact_value(v, secrets)).collect()),
        other => other.clone(),
    }
}

fn redact_kind_error(err: KindError, secrets: &[String]) -> KindError {
    if secrets.is_empty() {
        return err;
    }
    match err {
        KindError::Structured {
            code,
            message,
            data,
        } => KindError::Structured {
            code,
            message: redact_str(&message, secrets),
            data: redact_value(&data, secrets),
        },
        other => other,
    }
}

fn outcome_usage(outcome: &SandboxOutcome) -> (i64, i64) {
    match outcome {
        SandboxOutcome::Exited {
            cpu_ms,
            peak_rss_kb,
            ..
        }
        | SandboxOutcome::NonZeroExit {
            cpu_ms,
            peak_rss_kb,
            ..
        }
        | SandboxOutcome::Signaled {
            cpu_ms,
            peak_rss_kb,
            ..
        }
        | SandboxOutcome::TimedOut {
            cpu_ms,
            peak_rss_kb,
        } => (*cpu_ms, *peak_rss_kb),
    }
}

// ---- the kind ---------------------------------------------------------------

pub struct PythonKind {
    envs_root: PathBuf,
    scratch_root: PathBuf,
    isolation: IsolationMechanism,
    envs: Arc<EnvRegistry>,
    semaphore: Arc<Semaphore>,
    cpu_budget: CpuBudget,
    warm: Arc<WarmPool>,
    /// PRD-mcphost-sandbox-ready: this kind's sandbox self-test state.
    selftest: Arc<SandboxSelftest>,
}

impl PythonKind {
    /// Production constructor: `data_dir` is `$MCPHOST_DATA_DIR`; envs live
    /// under `<data_dir>/envs`, per-call scratch dirs under
    /// `<data_dir>/scratch`. Mechanism auto-detected (bwrap preferred).
    /// Requirement 1: the warm pool's three env-configurable bounds
    /// (`MCPHOST_PY_WARM_TTL`/`_PER_TENANT`/`_MAX`) are read here, once, at
    /// startup -- same pattern as every other `MCPHOST_*` variable this
    /// crate reads in `main.rs`, just read locally since `PythonKind::new`
    /// is this kind's one production entry point.
    pub fn new(data_dir: &Path) -> Self {
        let ttl = std::env::var("MCPHOST_PY_WARM_TTL")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .map(Duration::from_secs)
            .unwrap_or(Duration::from_secs(DEFAULT_WARM_TTL_S));
        let per_tenant = std::env::var("MCPHOST_PY_WARM_PER_TENANT")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(DEFAULT_WARM_PER_TENANT);
        let max_total = std::env::var("MCPHOST_PY_WARM_MAX")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(DEFAULT_WARM_MAX);
        Self::build(
            data_dir,
            sandbox::detect_mechanism(),
            DEFAULT_MAX_CONCURRENT_CALLS,
            DEFAULT_CPU_BUDGET_MS_PER_HOUR,
            ttl,
            per_tenant,
            max_total,
        )
    }

    /// Test constructor: a small concurrency limit so AC13 (admission
    /// control) doesn't need 21 real sandboxed calls in flight to prove the
    /// 21st is refused.
    pub fn for_test_with_concurrency(data_dir: &Path, max_concurrent: usize) -> Self {
        Self::build(
            data_dir,
            sandbox::detect_mechanism(),
            max_concurrent,
            DEFAULT_CPU_BUDGET_MS_PER_HOUR,
            Duration::from_secs(DEFAULT_WARM_TTL_S),
            DEFAULT_WARM_PER_TENANT,
            DEFAULT_WARM_MAX,
        )
    }

    /// Test constructor: a tiny CPU budget so AC14 doesn't need 600 real
    /// CPU-seconds to prove the budget fires.
    pub fn for_test_with_cpu_budget_ms(data_dir: &Path, budget_ms: u64) -> Self {
        Self::build(
            data_dir,
            sandbox::detect_mechanism(),
            DEFAULT_MAX_CONCURRENT_CALLS,
            budget_ms,
            Duration::from_secs(DEFAULT_WARM_TTL_S),
            DEFAULT_WARM_PER_TENANT,
            DEFAULT_WARM_MAX,
        )
    }

    /// Test constructor: a short TTL and small bounds so the warm-pool ACs
    /// (AC1/AC2/AC5/AC8) don't need a real 60s wait or 16 real sandboxes.
    ///
    /// Also self-sufficient about `uv`: every warm-pool test drives a real
    /// cold build (`run_build_steps` spawns `uv venv`/`uv pip install`
    /// bare, relying on `$PATH`), and `uv`'s own installer puts it in
    /// `~/.local/bin`, which a *login* shell's rc file adds to `$PATH` but
    /// a bare non-login process (e.g. an `ssh host cmd` invocation, or a
    /// harness that execs `cargo test` directly) does not. Without this,
    /// these tests only pass by accident of whatever shell happened to
    /// launch `cargo test` -- see [`ensure_uv_discoverable_for_test`].
    pub fn for_test_with_warm_pool(
        data_dir: &Path,
        ttl: Duration,
        per_tenant: usize,
        max_total: usize,
    ) -> Self {
        ensure_uv_discoverable_for_test();
        Self::build(
            data_dir,
            sandbox::detect_mechanism(),
            DEFAULT_MAX_CONCURRENT_CALLS,
            DEFAULT_CPU_BUDGET_MS_PER_HOUR,
            ttl,
            per_tenant,
            max_total,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn build(
        data_dir: &Path,
        isolation: IsolationMechanism,
        max_concurrent: usize,
        cpu_budget_ms: u64,
        warm_ttl: Duration,
        warm_per_tenant: usize,
        warm_max: usize,
    ) -> Self {
        let warm = Arc::new(WarmPool::new(warm_ttl, warm_per_tenant, warm_max));
        spawn_warm_reaper(warm.clone());
        let selftest = SandboxSelftest::new(isolation, warm.clone());
        Self {
            envs_root: data_dir.join("envs"),
            scratch_root: data_dir.join("scratch"),
            isolation,
            envs: Arc::new(EnvRegistry::new()),
            semaphore: Arc::new(Semaphore::new(max_concurrent)),
            cpu_budget: CpuBudget::new(cpu_budget_ms),
            warm,
            selftest,
        }
    }

    /// PRD-mcphost-sandbox-ready: a python kind whose sandbox self-test
    /// interpreter and periodic-recheck interval are both test-controlled
    /// (`tests/sandboxready_ac*.rs`). `interpreter` starts as
    /// `SelftestInterpreter::RealPython3`; use
    /// [`Self::set_selftest_interpreter_for_test`] to swap it (AC5/AC6:
    /// "swap the injected interpreter back to a working one" / "replaced by
    /// a failing one") without re-registering the kind. `recheck_secs`
    /// governs the periodic loop's interval in *both* the ready and
    /// unready states (see `SandboxSelftest::interval`'s doc comment for
    /// why one instance-level override, not the production env var, is how
    /// this crate's tests exercise a fast periodic recheck).
    ///
    /// PRD-mcphost-classify-precision: gated behind `cfg(test)` (this
    /// crate's own unit tests) or the `test-support` feature (this crate's
    /// `tests/*.rs` integration tests, which link the lib built *without*
    /// `--cfg test` -- see the `[dev-dependencies]` self-reference in
    /// `Cargo.toml`) so it is not reachable from an ordinary
    /// default-features consumer of this lib.
    #[cfg(any(test, feature = "test-support"))]
    pub fn for_test_with_selftest(data_dir: &Path, recheck_secs: u64) -> Self {
        let mut kind = Self::build(
            data_dir,
            sandbox::detect_mechanism(),
            DEFAULT_MAX_CONCURRENT_CALLS,
            DEFAULT_CPU_BUDGET_MS_PER_HOUR,
            Duration::from_secs(DEFAULT_WARM_TTL_S),
            DEFAULT_WARM_PER_TENANT,
            DEFAULT_WARM_MAX,
        );
        let warm = kind.warm.clone();
        kind.selftest = Arc::new(SandboxSelftest {
            status: RwLock::new(sandbox::SandboxStatus {
                ready: true,
                mechanism: kind.isolation,
                detail: format!("{}: not yet checked", kind.isolation.as_str()),
                checked_at: crate::state::rfc3339_now(),
            }),
            isolation: kind.isolation,
            interpreter: RwLock::new(SelftestInterpreter::RealPython3),
            recheck_unready_secs: recheck_secs,
            recheck_ready_secs_override: Some(recheck_secs),
            warm,
        });
        kind
    }

    /// Test-only: swaps the sandbox self-test's interpreter (see
    /// `SelftestInterpreter`'s doc comment) without touching any other
    /// state. `body` is a shell script written verbatim into the probe's
    /// own scratch dir on the next probe -- pass `None` to switch back to
    /// the real `/usr/bin/python3` (AC5's "swap the injected interpreter
    /// back to a working one"), or a script that writes to stderr and exits
    /// non-zero to simulate a specific failure (AC1/AC6/AC9).
    ///
    /// PRD-mcphost-classify-precision: gated the same way as
    /// [`Self::for_test_with_selftest`] (`cfg(test)` or `test-support`).
    #[cfg(any(test, feature = "test-support"))]
    pub fn set_selftest_interpreter_for_test(&self, body: Option<&str>) {
        let interpreter = match body {
            None => SelftestInterpreter::RealPython3,
            Some(script) => SelftestInterpreter::Fake(script.to_string()),
        };
        self.selftest.set_interpreter(interpreter);
    }

    /// Test-only (AC9's "missing bwrap" case): the next probe's `RunSpec`
    /// fails at `Command::spawn()` itself.
    ///
    /// PRD-mcphost-classify-precision: gated the same way as
    /// [`Self::for_test_with_selftest`] (`cfg(test)` or `test-support`).
    #[cfg(any(test, feature = "test-support"))]
    pub fn set_selftest_missing_binary_for_test(&self) {
        self.selftest
            .set_interpreter(SelftestInterpreter::MissingBinarySpawnFailure);
    }

    /// PRD-mcphost-sandbox-ready requirement 1/5: runs the startup
    /// self-test synchronously (`main.rs` awaits this before the HTTP
    /// listener starts accepting connections), records the result, logs the
    /// required journal line, and spawns the periodic re-probe task
    /// (requirement 4). Never fails startup: an unready sandbox is a
    /// recorded status, not an `Err` -- the host still serves `echo` and
    /// `http` regardless (requirement 1).
    pub async fn run_startup_selftest(&self) -> sandbox::SandboxStatus {
        self.selftest.startup().await
    }

    /// PRD-mcphost-sandbox-ready requirement 4: `admin.sandbox_recheck`'s
    /// on-demand path -- re-runs the self-test immediately and returns the
    /// fresh status.
    pub async fn recheck_sandbox(&self) -> sandbox::SandboxStatus {
        self.selftest.recheck().await
    }

    /// The most recently recorded sandbox self-test result, on the concrete
    /// type (tests hold `PythonKind` directly before it's registered as
    /// `Arc<dyn Kind>`). Named distinctly from the `Kind` trait's own
    /// `sandbox_status` (which returns `Option<_>`, `None` for kinds with
    /// no self-test) to avoid inherent-vs-trait-method resolution surprises
    /// on the same type.
    pub fn current_sandbox_status(&self) -> sandbox::SandboxStatus {
        self.selftest.read_status()
    }

    /// The active isolation mechanism, for `/healthz` (requirement 5).
    pub fn mechanism(&self) -> &'static str {
        self.isolation.as_str()
    }

    /// Diagnostic access to the warm pool's own counters -- `(hits, misses,
    /// current_pool_size)` (module doc: requirement 4's metrics, tracked
    /// here but not yet piped through `host.usage`/`admin.usage`). `pub`
    /// rather than test-only so this crate's own integration tests
    /// (`tests/warmpool_ac*.rs`) can assert a call actually hit the pool
    /// rather than inferring it indirectly from timing alone.
    pub fn warm_metrics(&self) -> (u64, u64, usize) {
        self.warm.metrics()
    }

    fn env_dir(&self, namespace: &str, requirements: &[String]) -> PathBuf {
        self.envs_root
            .join(namespace)
            .join(requirements_hash(requirements))
    }

    async fn prepare_scratch(
        &self,
        source: &str,
        args: &Value,
        site_packages: &str,
    ) -> Result<PathBuf, KindError> {
        let scratch = self.scratch_root.join(format!(
            "call-{}-{}",
            std::process::id(),
            rand::random::<u64>()
        ));
        tokio::fs::create_dir_all(&scratch)
            .await
            .map_err(|e| KindError::Exec(format!("scratch dir: {e}")))?;
        tokio::fs::write(scratch.join("tool.py"), source)
            .await
            .map_err(|e| KindError::Exec(format!("write tool source: {e}")))?;
        tokio::fs::write(scratch.join("runner.py"), PY_RUNNER_SCRIPT)
            .await
            .map_err(|e| KindError::Exec(format!("write runner: {e}")))?;
        let payload = json!({"args": args, "site_packages": site_packages});
        tokio::fs::write(
            scratch.join("stdin.json"),
            serde_json::to_vec(&payload).unwrap_or_default(),
        )
        .await
        .map_err(|e| KindError::Exec(format!("write stdin payload: {e}")))?;
        Ok(scratch)
    }

    /// Same idea as [`Self::prepare_scratch`], but for a sandbox meant to
    /// outlive one call (the warm pool): no per-call `stdin.json` (args
    /// arrive later, per call, over the sandbox's own stdin pipe -- see
    /// [`sandbox::PersistentSandbox::call`]), and the directory is named
    /// `warm-*` rather than `call-*` so an operator inspecting
    /// `<data_dir>/scratch` can tell the two apart.
    async fn prepare_scratch_for_warm(&self, source: &str) -> Result<PathBuf, KindError> {
        let scratch = self.scratch_root.join(format!(
            "warm-{}-{}",
            std::process::id(),
            rand::random::<u64>()
        ));
        tokio::fs::create_dir_all(&scratch)
            .await
            .map_err(|e| KindError::Exec(format!("warm scratch dir: {e}")))?;
        tokio::fs::write(scratch.join("tool.py"), source)
            .await
            .map_err(|e| KindError::Exec(format!("write tool source: {e}")))?;
        tokio::fs::write(scratch.join("runner.py"), PY_RUNNER_SCRIPT)
            .await
            .map_err(|e| KindError::Exec(format!("write runner: {e}")))?;
        Ok(scratch)
    }

    fn network_mode(&self, spec: &PythonSpec) -> NetworkMode {
        if spec.effective_network() == "public" {
            NetworkMode::Public {
                http_proxy: std::env::var("MCPHOST_EGRESS_PROXY").ok(),
            }
        } else {
            NetworkMode::None
        }
    }

    fn secret_env(&self, spec: &PythonSpec, ctx: &CallCtx) -> Vec<(String, String)> {
        spec.secrets
            .iter()
            .filter_map(|name| {
                ctx.secrets
                    .resolve(name)
                    .map(|value| (format!("SECRET_{}", name.to_ascii_uppercase()), value))
            })
            .collect()
    }

    /// Requirement 1 (AC1/AC4): attempts a warm reuse for `key`. `Some(_)`
    /// means the warm path fully answered this call (a hit that ran, or a
    /// definitive failure like a timeout) -- the caller should return it
    /// as-is. `None` means "no usable warm sandbox" (nothing pooled, a
    /// stale fingerprint, or the pooled sandbox turned out to be dead) --
    /// the caller falls through to the ordinary cold path, which is itself
    /// responsible for re-seeding the pool afterward.
    #[allow(clippy::too_many_arguments)]
    async fn try_warm(
        &self,
        key: &ToolKey,
        fingerprint: &str,
        args: &Value,
        timeout: Duration,
        ctx: &CallCtx,
        effective_schema: &Value,
        secret_values: &[String],
    ) -> Option<Result<Value, KindError>> {
        let mut entry = self.warm.take(key)?;
        if entry.fingerprint != fingerprint {
            // Requirement 1: "any other call starts cold" -- a different
            // source/requirements pairing under the same tool name (a
            // republish `on_tool_changed` should already have caught
            // synchronously, this is the defensive fallback) is not this
            // call's sandbox to reuse.
            kill_warm_entry(entry).await;
            return None;
        }

        let payload = call_payload(args, &entry.site_packages);
        let outcome = entry.sandbox.call(&payload, timeout).await;
        match outcome {
            Ok(PersistentCallOutcome::Responded {
                line,
                cpu_ms,
                peak_rss_kb,
            }) => {
                self.warm.record_hit();
                ctx.resources.record(cpu_ms, peak_rss_kb);
                self.cpu_budget.record(ctx.tenant_id, cpu_ms);
                let result = map_envelope_line(&line);
                entry.last_used = Instant::now();
                self.warm.offer(key.clone(), entry).await;
                Some(match result {
                    Ok(value) => {
                        let redacted = redact_value(&value, secret_values);
                        if ctx.test_mode {
                            Ok(json!({"result": redacted, "schema": effective_schema}))
                        } else {
                            Ok(redacted)
                        }
                    }
                    Err(e) => Err(redact_kind_error(e, secret_values)),
                })
            }
            // AC5: the deadline passed -- `call_timeout`, the sandbox is
            // killed, and (since `entry` is simply dropped, never offered
            // back) the pool no longer holds it.
            Ok(PersistentCallOutcome::TimedOut) => {
                kill_warm_entry(entry).await;
                Some(Err(KindError::structured(
                    "tool_timeout",
                    "the call exceeded its timeout",
                )))
            }
            // The sandbox died (crashed, hit its own rlimit, or was killed
            // out of band) between being pooled and this call reaching it --
            // not a hit, but also not the caller's fault; fall through to an
            // ordinary cold call rather than surfacing a confusing error for
            // something the pool itself should recover from transparently.
            Ok(PersistentCallOutcome::Closed) | Err(_) => {
                kill_warm_entry(entry).await;
                None
            }
        }
    }

    /// Requirement 1/6 (AC1/AC8): spawns a fresh, idle [`PersistentSandbox`]
    /// running the same source and requirements this cold call just used,
    /// and offers it to the warm pool -- best-effort. Any failure here
    /// (spawn error, pool already full by the time this runs) just means
    /// the next call is cold too; it never affects the cold call already in
    /// progress, whose result is computed independently of this.
    #[allow(clippy::too_many_arguments)]
    async fn maybe_promote_to_warm(
        &self,
        key: ToolKey,
        fingerprint: String,
        parsed: &PythonSpec,
        python: PathBuf,
        site_packages: String,
        env_dir: PathBuf,
        secret_env: &[(String, String)],
    ) {
        if !self.warm.has_room(key.0) {
            return;
        }
        let Ok(scratch) = self.prepare_scratch_for_warm(&parsed.source).await else {
            return;
        };
        let mut read_only_dirs = system_python_dirs();
        read_only_dirs.push(env_dir);

        let run_spec = sandbox::RunSpec {
            interpreter: python,
            interpreter_args: vec!["-I".to_string(), "-S".to_string()],
            script_path: scratch.join("runner.py"),
            scratch_dir: scratch.clone(),
            read_only_dirs,
            stdin_payload: Vec::new(),
            limits: ResourceLimits {
                // Module doc's "warm-pool RLIMIT_CPU" note: this is a
                // process-lifetime kernel backstop, not the per-call limit
                // (that's the wall-clock timeout `try_warm` already passes
                // to every `PersistentSandbox::call`).
                cpu_seconds: parsed.effective_timeout_s().saturating_mul(1000),
                memory_mb: parsed.effective_memory_mb(),
                max_open_files: MAX_OPEN_FILES,
                max_file_size_mb: MAX_FILE_SIZE_MB,
            },
            wall_clock_timeout: Duration::from_secs(parsed.effective_timeout_s() + 2),
            network: self.network_mode(parsed),
            extra_env: secret_env.to_vec(),
            isolation: self.isolation,
        };

        match sandbox::spawn_persistent(&run_spec).await {
            Ok(sandbox) => {
                let entry = WarmEntry {
                    sandbox,
                    fingerprint,
                    site_packages,
                    scratch_dir: scratch,
                    last_used: Instant::now(),
                };
                self.warm.offer(key, entry).await;
            }
            Err(_) => {
                let _ = tokio::fs::remove_dir_all(&scratch).await;
            }
        }
    }
}

#[async_trait::async_trait]
impl Kind for PythonKind {
    fn name(&self) -> &'static str {
        "python"
    }

    fn validate(&self, spec: &Value) -> Result<(), KindError> {
        let parsed = parse_spec(spec)?;
        validate_spec_fields(&parsed)
    }

    fn validate_all(&self, spec: &Value) -> Vec<KindError> {
        match parse_spec(spec) {
            Ok(parsed) => validate_spec_fields_all(&parsed),
            Err(e) => vec![e],
        }
    }

    async fn validate_async(&self, spec: &Value) -> Result<(), KindError> {
        // PRD-mcphost-python-kind-runtime requirement 1/AC4: instrument
        // publish latency end-to-end (spec parse, the sandboxed AST check,
        // and schema/requirements inference when the tenant omitted
        // either) -- this is the entire cost of `host.tool_publish` for a
        // python-kind tool, and the number the AC's ≤10s budget is judged
        // against. The `async {}` block lets every `?` inside still early-
        // return normally while one `tracing::info!` after it covers both
        // the success and failure path -- a rejected publish still cost
        // real wall time worth recording.
        let started = Instant::now();
        let result = async {
            let parsed = parse_spec(spec)?;
            // AC16: a syntax error must surface as the existing spec-validation
            // error, never an inference error -- so this runs first, and
            // inference (below) is only reached once the source is known-valid
            // Python with a `main`.
            ast_check(&parsed.source, self.isolation).await?;
            // Requirement 7/AC11 and requirement 6/AC10: gate publish on
            // inference actually succeeding when the tenant omitted the field.
            // The derived value itself is discarded here -- `describe`/`call`
            // recompute it deterministically (see `infer` module docs).
            if parsed.args_schema.is_none() {
                infer::infer_python_args_schema(&parsed.source)?;
            }
            if parsed.requirements.is_empty() {
                infer::infer_python_requirements(&parsed.source)?;
            }
            Ok(())
        }
        .await;
        tracing::info!(
            publish_ms = started.elapsed().as_millis() as u64,
            ok = result.is_ok(),
            "python publish (validate_async) complete"
        );
        result
    }

    fn describe(&self, spec: &Value) -> ToolDescriptor {
        match parse_spec(spec) {
            Ok(parsed) => {
                // Requirement 9: an inferred tool is indistinguishable from
                // an authored one in `tools/list`. A source whose inference
                // would fail can't have been published (validate_async
                // gated it), so this fallback is unreachable in practice;
                // it exists so `describe` never panics on a spec that
                // somehow bypassed publish (e.g. a future direct DB edit).
                let input_schema = parsed
                    .effective_args_schema()
                    .unwrap_or_else(|_| json!({"type": "object"}));
                ToolDescriptor {
                    name: "python".to_string(),
                    description: parsed
                        .description
                        .clone()
                        .unwrap_or_else(|| "Runs a tenant-published Python function.".to_string()),
                    input_schema,
                }
            }
            Err(e) => ToolDescriptor {
                name: "python".to_string(),
                description: format!("invalid python tool spec: {e}"),
                input_schema: json!({"type": "object"}),
            },
        }
    }

    fn referenced_secrets(&self, spec: &Value) -> Vec<String> {
        parse_spec(spec).map(|p| p.secrets).unwrap_or_default()
    }

    fn declared_outputs(&self, spec: &Value) -> Vec<String> {
        parse_spec(spec).map(|p| p.outputs).unwrap_or_default()
    }

    fn payload_from_call_result<'a>(&self, call_result: &'a Value) -> Option<&'a Value> {
        // Requirement 10 / AC15's test-mode echo nests the ordinary result
        // (and its `payload`) under `result` -- see `call`'s
        // `ctx.test_mode` branch above.
        call_result
            .get("payload")
            .or_else(|| call_result.get("result").and_then(|r| r.get("payload")))
    }

    /// PRD-mcphost-tool-test AC1: `host.spec_test`'s reported `requirements`
    /// for a python spec -- the author's own, or inferred from `source`,
    /// exactly like [`Kind::call`]/`describe` would build the environment
    /// with (`PythonSpec::effective_requirements`). `Vec::new()` on an
    /// unparseable spec: `spec_test`'s own `validate_all`/`validate_async`
    /// gate already reject that spec before this is ever reached in
    /// practice, so this fallback exists only so `requirements` never panics.
    fn requirements(&self, spec: &Value) -> Vec<String> {
        parse_spec(spec)
            .and_then(|parsed| parsed.effective_requirements())
            .unwrap_or_default()
    }

    async fn call(&self, spec: &Value, args: Value, ctx: &CallCtx) -> Result<Value, KindError> {
        let parsed = parse_spec(spec)?;
        validate_spec_fields(&parsed)?;

        let effective_schema = parsed.effective_args_schema()?;
        let validator = jsonschema::validator_for(&effective_schema)
            .map_err(|e| KindError::InvalidSpec(format!("args_schema: {e}")))?;
        if let Err(e) = validator.validate(&args) {
            // Requirement 1/AC3: phase `args_coercion`, naming the
            // argument and both types -- see `describe_args_error`.
            let data = super::describe_args_error(&e);
            return Err(KindError::structured_with("args_invalid", e.to_string(), data));
        }

        let Ok(_permit) = self.semaphore.clone().try_acquire_owned() else {
            return Err(KindError::structured(
                "capacity",
                "at the concurrent-call limit; try again shortly",
            ));
        };

        let secret_env = self.secret_env(&parsed, ctx);
        let secret_values: Vec<String> = secret_env.iter().map(|(_, v)| v.clone()).collect();

        if let Err(retry_after_s) = self.cpu_budget.check(ctx.tenant_id) {
            return Err(KindError::structured_with(
                "rate_limited",
                "tenant CPU budget exceeded for this hour",
                json!({"retry_after_s": retry_after_s}),
            ));
        }

        let effective_requirements = parsed.effective_requirements()?;

        // Requirement 1 (AC1/AC4/AC8): a repeat call of the same tool with
        // the same requirements/source reuses a warm sandbox if one is
        // idle, skipping the env-build-status lookup and the cold spawn
        // entirely. `ctx.tool_name` is `None` in every context that has no
        // notion of a tool's own name (`for_test`, the conformance suite) --
        // those always fall through to the cold path below, by design (see
        // the module doc's "Pool key" note).
        let warm_key: Option<ToolKey> = ctx.tool_name.clone().map(|name| (ctx.tenant_id, name));
        let fingerprint = call_fingerprint(&parsed.source, &effective_requirements, &secret_env);
        if let Some(key) = &warm_key {
            let call_timeout = Duration::from_secs(parsed.effective_timeout_s() + 2);
            if let Some(result) = self
                .try_warm(
                    key,
                    &fingerprint,
                    &args,
                    call_timeout,
                    ctx,
                    &effective_schema,
                    &secret_values,
                )
                .await
            {
                return result;
            }
            // `try_warm` returning `None` covers every "no usable warm
            // sandbox" reason (nothing pooled yet, a stale fingerprint, or
            // a pooled sandbox that turned out to be dead) -- all of them
            // are a miss for requirement 4's metrics; only a `Some(_)`
            // return above (an actual hit, or a definitive failure like
            // AC5's timeout) is not.
            self.warm.record_miss();
        }

        // PRD-mcphost-python-kind-runtime requirement 1/AC4: everything
        // from here down only runs on the cold path (a warm-pool hit
        // above already returned) -- this is exactly "first call" cost:
        // env-status lookup, scratch prep, and the sandboxed run itself.
        // Logged unconditionally (including the `tool_building`/
        // `build_failed` early returns) since even a rejected cold call
        // spent real wall time getting there.
        let cold_call_started = Instant::now();
        let env_dir = self.env_dir(&ctx.namespace, &effective_requirements);
        let status = match self.envs.status(&env_dir).await {
            Some(status) => status,
            None => {
                self.envs.start_build(
                    env_dir.clone(),
                    ctx.namespace.clone(),
                    effective_requirements,
                );
                return Err(KindError::structured(
                    "tool_building",
                    "the tool's environment is still building; try again shortly",
                ));
            }
        };
        let (python, site_packages) = match status {
            EnvStatus::Ready {
                python,
                site_packages,
            } => (python, site_packages),
            EnvStatus::Building => {
                return Err(KindError::structured(
                    "tool_building",
                    "the tool's environment is still building; try again shortly",
                ));
            }
            EnvStatus::Failed { tail } => {
                return Err(KindError::structured_with(
                    "build_failed",
                    "the tool's environment failed to build",
                    json!({"tail": tail}),
                ));
            }
        };

        let scratch = self
            .prepare_scratch(&parsed.source, &args, &site_packages)
            .await?;
        let stdin_payload = tokio::fs::read(scratch.join("stdin.json"))
            .await
            .map_err(|e| KindError::Exec(format!("read stdin payload: {e}")))?;

        let mut read_only_dirs = system_python_dirs();
        read_only_dirs.push(env_dir.clone());

        // Cloned before `run_spec` below moves it: requirement 1/AC1's
        // pre-warm-after-a-cold-miss step (`maybe_promote_to_warm`) needs
        // the same interpreter path once this cold call is done with it.
        let python_for_warm = python.clone();

        let run_spec = sandbox::RunSpec {
            interpreter: python,
            interpreter_args: vec!["-I".to_string(), "-S".to_string()],
            script_path: scratch.join("runner.py"),
            scratch_dir: scratch.clone(),
            read_only_dirs,
            stdin_payload,
            limits: ResourceLimits {
                cpu_seconds: parsed.effective_timeout_s(),
                memory_mb: parsed.effective_memory_mb(),
                max_open_files: MAX_OPEN_FILES,
                max_file_size_mb: MAX_FILE_SIZE_MB,
            },
            wall_clock_timeout: Duration::from_secs(parsed.effective_timeout_s() + 2),
            network: self.network_mode(&parsed),
            extra_env: secret_env.clone(),
            isolation: self.isolation,
        };

        let outcome = sandbox::run(run_spec).await;
        let _ = tokio::fs::remove_dir_all(&scratch).await;
        // Requirement 1/AC4: the cold-path number the AC's ≤5s budget is
        // judged against, logged before the spawn-error `?` so a genuine
        // spawn failure is still timed and visible.
        tracing::info!(
            cold_call_ms = cold_call_started.elapsed().as_millis() as u64,
            "python cold call (env lookup + sandboxed run) complete"
        );
        let outcome = outcome.map_err(|e| KindError::Exec(format!("sandbox spawn failed: {e}")))?;

        let (cpu_ms, peak_rss_kb) = outcome_usage(&outcome);
        ctx.resources.record(cpu_ms, peak_rss_kb);
        self.cpu_budget.record(ctx.tenant_id, cpu_ms);

        // Requirement 1/AC1/AC8: a cold call that ran the sandboxed process
        // to completion (whether the tool itself succeeded or raised -- a
        // healthy sandbox either way) is a candidate to seed the warm pool
        // for the *next* call, if this call came through a real dispatch
        // path (`warm_key.is_some()`) and there's room. Anything else that
        // could have gone wrong (spawn failure already returned above; a
        // timeout, signal or non-zero exit below) means the sandbox itself
        // is not healthy enough to keep alive.
        if let (Some(key), true) = (&warm_key, matches!(&outcome, SandboxOutcome::Exited { .. })) {
            self.maybe_promote_to_warm(
                key.clone(),
                fingerprint.clone(),
                &parsed,
                python_for_warm,
                site_packages,
                env_dir,
                &secret_env,
            )
            .await;
        }

        // Requirement 7: secret values are "redacted from any string that
        // leaves the host" -- that includes a tool's own result (a tool may
        // legitimately be handed a secret and choose to echo it back, e.g.
        // while debugging), not just an error/traceback.
        match map_sandbox_outcome(outcome) {
            Ok(value) => {
                let redacted = redact_value(&value, &secret_values);
                // PRD-mcphost-result-envelope-contract requirement 1/3,
                // AC2/AC3: declared output fields promoted to
                // `result.payload.<field>` -- a no-op when this spec
                // declares none.
                let redacted = apply_declared_outputs(redacted, &parsed.outputs);
                // Requirement 10 / AC15: `host.tool_test` shows the schema
                // the host inferred (or the author's own, unchanged) so a
                // tenant can inspect it before relying on it. Same pattern
                // `http`'s own `call` already uses for its test-mode-only
                // request echo -- attach the debug info to the response
                // `Value` itself, never touching `handler.rs`.
                if ctx.test_mode {
                    Ok(json!({"result": redacted, "schema": effective_schema}))
                } else {
                    Ok(redacted)
                }
            }
            Err(e) => Err(redact_kind_error(e, &secret_values)),
        }
    }

    /// `host.tool_run` (requirement 3, AC6/AC7). Always cold (module doc:
    /// "host.tool_run execution path") -- never touches the warm pool, so a
    /// debug run can never evict or block on a sandbox serving real
    /// traffic. Writes no `calls` row (this method is never reached from
    /// `call_published_tool`), records no CPU-budget usage and does not
    /// acquire the admission-control semaphore (its own 30/minute rate
    /// limit, enforced in `handler.rs` before this is ever called, is the
    /// intended throughput bound for this RPC).
    async fn tool_run(&self, spec: &Value, args: Value, ctx: &CallCtx) -> Result<Value, KindError> {
        let parsed = parse_spec(spec)?;
        validate_spec_fields(&parsed)?;

        let effective_schema = parsed.effective_args_schema()?;
        let validator = jsonschema::validator_for(&effective_schema)
            .map_err(|e| KindError::InvalidSpec(format!("args_schema: {e}")))?;
        if let Err(e) = validator.validate(&args) {
            // Requirement 1/AC3: phase `args_coercion`, naming the
            // argument and both types -- see `describe_args_error`.
            let data = super::describe_args_error(&e);
            return Err(KindError::structured_with("args_invalid", e.to_string(), data));
        }

        let secret_env = self.secret_env(&parsed, ctx);
        let secret_values: Vec<String> = secret_env.iter().map(|(_, v)| v.clone()).collect();

        let effective_requirements = parsed.effective_requirements()?;
        let env_dir = self.env_dir(&ctx.namespace, &effective_requirements);
        let status = match self.envs.status(&env_dir).await {
            Some(status) => status,
            None => {
                self.envs.start_build(
                    env_dir.clone(),
                    ctx.namespace.clone(),
                    effective_requirements,
                );
                return Err(KindError::structured(
                    "tool_building",
                    "the tool's environment is still building; try again shortly",
                ));
            }
        };
        let (python, site_packages) = match status {
            EnvStatus::Ready {
                python,
                site_packages,
            } => (python, site_packages),
            EnvStatus::Building => {
                return Err(KindError::structured(
                    "tool_building",
                    "the tool's environment is still building; try again shortly",
                ));
            }
            EnvStatus::Failed { tail } => {
                return Err(KindError::structured_with(
                    "build_failed",
                    "the tool's environment failed to build",
                    json!({"tail": tail}),
                ));
            }
        };

        let scratch = self
            .prepare_scratch(&parsed.source, &args, &site_packages)
            .await?;
        let stdin_payload = tokio::fs::read(scratch.join("stdin.json"))
            .await
            .map_err(|e| KindError::Exec(format!("read stdin payload: {e}")))?;
        let mut read_only_dirs = system_python_dirs();
        read_only_dirs.push(env_dir);

        let run_spec = sandbox::RunSpec {
            interpreter: python,
            interpreter_args: vec!["-I".to_string(), "-S".to_string()],
            script_path: scratch.join("runner.py"),
            scratch_dir: scratch.clone(),
            read_only_dirs,
            stdin_payload,
            limits: ResourceLimits {
                cpu_seconds: parsed.effective_timeout_s(),
                memory_mb: parsed.effective_memory_mb(),
                max_open_files: MAX_OPEN_FILES,
                max_file_size_mb: MAX_FILE_SIZE_MB,
            },
            wall_clock_timeout: Duration::from_secs(parsed.effective_timeout_s() + 2),
            network: self.network_mode(&parsed),
            extra_env: secret_env.clone(),
            isolation: self.isolation,
        };

        let outcome = sandbox::run(run_spec).await;
        let _ = tokio::fs::remove_dir_all(&scratch).await;
        let outcome = outcome.map_err(|e| KindError::Exec(format!("sandbox spawn failed: {e}")))?;

        Ok(redact_value(
            &tool_run_response(outcome, &effective_schema, ctx.test_mode),
            &secret_values,
        ))
    }

    /// PRD-mcphost-code-tools-warm-pool AC3: republish/remove of a python
    /// tool kills any warm sandbox for it before the RPC returns.
    async fn on_tool_changed(&self, tenant_id: i64, local_name: &str) {
        self.warm.evict_tool(tenant_id, local_name).await;
    }

    /// AC3: a disabled/deleted tenant's warm sandboxes don't outlive it.
    async fn on_tenant_removed(&self, tenant_id: i64) {
        self.warm.evict_tenant(tenant_id).await;
    }

    fn example(&self) -> KindExample {
        // PRD-mcphost-publish-first-try requirement 6 / AC6: sourced from
        // `docs/kinds/python.md`, not hand-duplicated here -- see
        // `crate::kinds::docs`.
        super::docs::parse_kind_doc(include_str!("../../docs/kinds/python.md"))
    }

    /// PRD-mcphost-sandbox-ready requirement 1/3: `/healthz` and the
    /// publish-time readiness gate both read this.
    fn sandbox_status(&self) -> Option<sandbox::SandboxStatus> {
        Some(self.selftest.read_status())
    }

    /// PRD-mcphost-sandbox-ready requirement 4: `admin.sandbox_recheck`.
    async fn sandbox_recheck(&self) -> Option<sandbox::SandboxStatus> {
        Some(self.selftest.recheck().await)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_data_dir() -> PathBuf {
        std::env::temp_dir().join(format!(
            "mcphost-python-kind-test-{}-{}",
            std::process::id(),
            rand::random::<u64>()
        ))
    }

    #[test]
    fn requirement_allowlist_rejects_urls_and_paths() {
        assert!(requirement_is_allowed("pydantic"));
        assert!(requirement_is_allowed("pydantic>=2.0"));
        assert!(!requirement_is_allowed(
            "pydantic @ https://example.com/x.whl"
        ));
        assert!(!requirement_is_allowed("git+https://github.com/x/y"));
        assert!(!requirement_is_allowed("./local-package"));
        assert!(!requirement_is_allowed("/abs/path"));
    }

    #[test]
    fn validate_rejects_missing_main_detection_fields() {
        let kind = PythonKind::new(&temp_data_dir());
        let err = kind.validate(&json!({})).unwrap_err();
        assert!(matches!(err, KindError::InvalidSpec(_)));
    }

    #[tokio::test]
    async fn ast_check_rejects_source_without_main() {
        if !sandbox::supports_user_namespaces() {
            println!("{}", sandbox::USERNS_SKIP_MARKER);
            return;
        }
        let data_dir = temp_data_dir();
        let kind = PythonKind::new(&data_dir);
        let spec = json!({
            "source": "def not_main(args):\n    return args\n",
            "args_schema": {"type": "object"},
        });
        kind.validate(&spec).expect("cheap validation passes");
        let err = kind.validate_async(&spec).await.unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("source"), "error must name source, got: {msg}");
    }

    #[tokio::test]
    async fn ast_check_names_the_syntax_error_line() {
        if !sandbox::supports_user_namespaces() {
            println!("{}", sandbox::USERNS_SKIP_MARKER);
            return;
        }
        let data_dir = temp_data_dir();
        let kind = PythonKind::new(&data_dir);
        let spec = json!({
            "source": "def main(args):\n    return (\n",
            "args_schema": {"type": "object"},
        });
        let err = kind.validate_async(&spec).await.unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("line"), "error must name the line, got: {msg}");
    }

    // ---- PRD-mcphost-code-tools-warm-pool -----------------------------

    #[test]
    fn call_fingerprint_is_sensitive_to_source_requirements_and_secrets() {
        let a = call_fingerprint("def main(args):\n    return {}\n", &[], &[]);
        let b = call_fingerprint("def main(args):\n    return {'x': 1}\n", &[], &[]);
        assert_ne!(a, b, "different source must fingerprint differently");

        let c = call_fingerprint(
            "def main(args):\n    return {}\n",
            &["requests".into()],
            &[],
        );
        assert_ne!(a, c, "different requirements must fingerprint differently");

        let d = call_fingerprint(
            "def main(args):\n    return {}\n",
            &[],
            &[("SECRET_TOKEN".into(), "v1".into())],
        );
        assert_ne!(a, d, "different secret values must fingerprint differently");

        let e = call_fingerprint("def main(args):\n    return {}\n", &[], &[]);
        assert_eq!(a, e, "identical inputs must fingerprint identically");
    }

    fn warm_ctx(tenant_id: i64, namespace: &str, tool_name: &str) -> CallCtx {
        CallCtx {
            tenant_id,
            namespace: namespace.to_string(),
            secrets: Arc::new(crate::kinds::NoSecrets),
            deadline: Instant::now() + Duration::from_secs(30),
            log: Arc::new(crate::kinds::NullLog),
            test_mode: false,
            resources: Arc::new(crate::kinds::NullResourceSink),
            tool_name: Some(tool_name.to_string()),
        }
    }

    /// Calls `kind.call(spec, args, ctx)` once, transparently retrying past
    /// any `tool_building` responses (the first call against a fresh env
    /// always gets at least one) -- the unit-test equivalent of
    /// `tests/common::poll_until_ready`.
    async fn call_through_build(
        kind: &PythonKind,
        spec: &Value,
        args: Value,
        ctx: &CallCtx,
    ) -> Result<Value, KindError> {
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            match kind.call(spec, args.clone(), ctx).await {
                Err(KindError::Structured {
                    code: "tool_building",
                    ..
                }) if Instant::now() < deadline => {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
                other => return other,
            }
        }
    }

    /// AC1/AC8 — a repeat call of the same tool/requirements reuses the
    /// warm sandbox the first (cold) call seeded, with identical-shaped
    /// results for identical arguments.
    #[tokio::test]
    async fn repeat_call_is_a_warm_hit_ac1() {
        if !sandbox::supports_user_namespaces() {
            println!("{}", sandbox::USERNS_SKIP_MARKER);
            return;
        }
        let data_dir = temp_data_dir();
        let kind = PythonKind::for_test_with_warm_pool(&data_dir, Duration::from_secs(60), 2, 16);
        let spec = json!({
            "source": "def main(args):\n    return {\"n\": args[\"n\"] * 2}\n",
            "args_schema": {"type": "object"},
            "requirements": [],
        });
        let ctx = warm_ctx(1, "wpac1", "doubler");

        let first = call_through_build(&kind, &spec, json!({"n": 3}), &ctx)
            .await
            .expect("cold call must succeed");
        assert_eq!(first, json!({"n": 6}));

        let (hits_before, misses_before, pool_size) = kind.warm_metrics();
        assert!(misses_before >= 1, "the cold call must count as a miss");
        assert_eq!(pool_size, 1, "the cold call must have seeded the pool");

        let second = kind
            .call(&spec, json!({"n": 5}), &ctx)
            .await
            .expect("warm call must succeed");
        assert_eq!(second, json!({"n": 10}));

        let (hits_after, _, pool_size_after) = kind.warm_metrics();
        assert_eq!(
            hits_after,
            hits_before + 1,
            "the second call must be recorded as a warm hit"
        );
        assert_eq!(
            pool_size_after, 1,
            "the reused sandbox must be back in the pool afterward"
        );
    }

    /// AC2 — an idle warm sandbox past its TTL is reaped; the pool no
    /// longer holds it.
    #[tokio::test]
    async fn idle_warm_entry_is_reaped_after_ttl_ac2() {
        if !sandbox::supports_user_namespaces() {
            println!("{}", sandbox::USERNS_SKIP_MARKER);
            return;
        }
        let data_dir = temp_data_dir();
        let ttl = Duration::from_millis(200);
        let kind = PythonKind::for_test_with_warm_pool(&data_dir, ttl, 2, 16);
        let spec = json!({
            "source": "def main(args):\n    return {\"ok\": True}\n",
            "args_schema": {"type": "object"},
            "requirements": [],
        });
        let ctx = warm_ctx(2, "wpac2", "pinger");

        call_through_build(&kind, &spec, json!({}), &ctx)
            .await
            .expect("cold call must succeed");
        assert_eq!(
            kind.warm_metrics().2,
            1,
            "the cold call must have seeded the pool"
        );

        // Exercises `WarmPool::reap_expired` directly rather than waiting
        // out the real (5s-interval) background reaper task -- same
        // behavior, without a slow test.
        tokio::time::sleep(ttl + Duration::from_millis(100)).await;
        kind.warm.reap_expired().await;
        assert_eq!(
            kind.warm_metrics().2,
            0,
            "an idle entry past its TTL must be reaped"
        );
    }

    /// AC3 — a republish or removal kills the warm sandbox synchronously,
    /// via `Kind::on_tool_changed`; a tenant removal does the same for
    /// every one of its tools via `Kind::on_tenant_removed`.
    #[tokio::test]
    async fn republish_remove_and_tenant_removal_evict_synchronously_ac3() {
        if !sandbox::supports_user_namespaces() {
            println!("{}", sandbox::USERNS_SKIP_MARKER);
            return;
        }
        let data_dir = temp_data_dir();
        let kind = PythonKind::for_test_with_warm_pool(&data_dir, Duration::from_secs(60), 2, 16);
        let spec = json!({
            "source": "def main(args):\n    return {}\n",
            "args_schema": {"type": "object"},
            "requirements": [],
        });

        let ctx_a = warm_ctx(3, "wpac3", "thing_a");
        call_through_build(&kind, &spec, json!({}), &ctx_a)
            .await
            .expect("cold call must succeed");
        assert_eq!(kind.warm_metrics().2, 1);
        kind.on_tool_changed(3, "thing_a").await;
        assert_eq!(
            kind.warm_metrics().2,
            0,
            "on_tool_changed (republish/remove) must evict the warm entry"
        );

        let ctx_b = warm_ctx(3, "wpac3", "thing_b");
        call_through_build(&kind, &spec, json!({}), &ctx_b)
            .await
            .expect("cold call must succeed");
        assert_eq!(kind.warm_metrics().2, 1);
        kind.on_tenant_removed(3).await;
        assert_eq!(
            kind.warm_metrics().2,
            0,
            "on_tenant_removed must evict every warm entry for that tenant"
        );
    }

    /// AC4 — a warm reuse never leaks a module-level global from one call
    /// to the next.
    #[tokio::test]
    async fn warm_reuse_does_not_leak_module_state_ac4() {
        if !sandbox::supports_user_namespaces() {
            println!("{}", sandbox::USERNS_SKIP_MARKER);
            return;
        }
        let data_dir = temp_data_dir();
        let kind = PythonKind::for_test_with_warm_pool(&data_dir, Duration::from_secs(60), 2, 16);
        let spec = json!({
            "source": "_STATE = {}\ndef main(args):\n    seen_before = \"seen\" in _STATE\n    _STATE[\"seen\"] = True\n    return {\"seen_before\": seen_before}\n",
            "args_schema": {"type": "object"},
            "requirements": [],
        });
        let ctx = warm_ctx(4, "wpac4", "stateful");

        let first = call_through_build(&kind, &spec, json!({}), &ctx)
            .await
            .expect("cold call must succeed");
        assert_eq!(first, json!({"seen_before": false}));

        let second = kind
            .call(&spec, json!({}), &ctx)
            .await
            .expect("warm call must succeed");
        assert_eq!(
            second,
            json!({"seen_before": false}),
            "a module global set in call one must be absent in call two"
        );
        assert_eq!(
            kind.warm_metrics().0,
            1,
            "the second call must actually have gone through the warm path"
        );
    }

    /// AC5 — a warm call that exceeds its deadline is `tool_timeout`, kills
    /// the sandbox promptly, and the pool no longer holds it afterward.
    #[tokio::test]
    async fn warm_call_past_deadline_times_out_and_is_evicted_ac5() {
        if !sandbox::supports_user_namespaces() {
            println!("{}", sandbox::USERNS_SKIP_MARKER);
            return;
        }
        let data_dir = temp_data_dir();
        let kind = PythonKind::for_test_with_warm_pool(&data_dir, Duration::from_secs(60), 2, 16);
        let spec = json!({
            "source": "import time\ndef main(args):\n    if args.get(\"slow\"):\n        time.sleep(5)\n    return {\"ok\": True}\n",
            "args_schema": {"type": "object"},
            "requirements": [],
            "timeout_s": 1,
        });
        let ctx = warm_ctx(5, "wpac5", "maybe_slow");

        call_through_build(&kind, &spec, json!({"slow": false}), &ctx)
            .await
            .expect("cold call must succeed");
        assert_eq!(
            kind.warm_metrics().2,
            1,
            "the fast call must have seeded the pool"
        );

        let started = Instant::now();
        let err = kind
            .call(&spec, json!({"slow": true}), &ctx)
            .await
            .expect_err("a warm call past its deadline must fail");
        let elapsed = started.elapsed();
        assert!(matches!(
            err,
            KindError::Structured {
                code: "tool_timeout",
                ..
            }
        ));
        assert!(
            elapsed < Duration::from_secs(4),
            "must return promptly after killing the warm sandbox, took {elapsed:?}"
        );
        assert_eq!(
            kind.warm_metrics().2,
            0,
            "the timed-out warm sandbox must no longer be in the pool"
        );
    }
}
