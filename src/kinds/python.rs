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

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::sync::Semaphore;

use super::infer;
use super::{CallCtx, Kind, KindError, KindExample, ToolDescriptor};
use crate::sandbox::{self, IsolationMechanism, NetworkMode, ResourceLimits, SandboxOutcome};

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

fn validate_requirements(reqs: &[String]) -> Result<(), KindError> {
    if reqs.len() > MAX_REQUIREMENTS {
        return Err(KindError::InvalidSpec(format!(
            "requirements: at most {MAX_REQUIREMENTS} entries; got {}",
            reqs.len()
        )));
    }
    for req in reqs {
        if !requirement_is_allowed(req) {
            return Err(KindError::structured_with(
                "requirement_not_allowed",
                format!("requirements: '{req}' is not an allowed PyPI requirement"),
                json!({"requirement": req}),
            ));
        }
    }
    Ok(())
}

fn validate_spec_fields(parsed: &PythonSpec) -> Result<(), KindError> {
    if parsed.source.is_empty() {
        return Err(KindError::InvalidSpec("source: must not be empty".into()));
    }
    if parsed.source.len() > MAX_SOURCE_BYTES {
        return Err(KindError::InvalidSpec(format!(
            "source: {} bytes, over the {MAX_SOURCE_BYTES}-byte limit",
            parsed.source.len()
        )));
    }
    validate_requirements(&parsed.requirements)?;
    // Requirement 1: an author-supplied schema is checked exactly as
    // before; an absent one is left to `validate_async` (and later,
    // `describe`/`call`) to infer -- inference needs the sandboxed AST
    // check to have run first (AC16), which this synchronous fn can't do.
    if let Some(schema) = &parsed.args_schema {
        jsonschema::validator_for(schema).map_err(|e| {
            KindError::InvalidSpec(format!("args_schema: not a valid JSON Schema: {e}"))
        })?;
    }
    if let Some(t) = parsed.timeout_s
        && !(1..=MAX_TIMEOUT_S).contains(&t)
    {
        return Err(KindError::InvalidSpec(format!(
            "timeout_s: must be between 1 and {MAX_TIMEOUT_S}; got {t}"
        )));
    }
    if let Some(m) = parsed.memory_mb
        && !(1..=MAX_MEMORY_MB).contains(&m)
    {
        return Err(KindError::InvalidSpec(format!(
            "memory_mb: must be between 1 and {MAX_MEMORY_MB}; got {m}"
        )));
    }
    if let Some(n) = &parsed.network
        && n != "none"
        && n != "public"
    {
        return Err(KindError::InvalidSpec(format!(
            "network: must be 'none' or 'public'; got '{n}'"
        )));
    }
    Ok(())
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

async fn ast_check(source: &str, isolation: IsolationMechanism) -> Result<(), KindError> {
    let scratch = std::env::temp_dir().join(format!(
        "mcphost-astcheck-{}-{}",
        std::process::id(),
        rand::random::<u64>()
    ));
    tokio::fs::create_dir_all(&scratch)
        .await
        .map_err(|e| KindError::Exec(format!("ast-check scratch dir: {e}")))?;
    let script_path = scratch.join("ast_check.py");
    tokio::fs::write(&script_path, AST_CHECK_SCRIPT)
        .await
        .map_err(|e| KindError::Exec(format!("ast-check script write: {e}")))?;

    let spec = sandbox::RunSpec {
        interpreter: PathBuf::from("/usr/bin/python3"),
        interpreter_args: vec!["-I".to_string(), "-S".to_string()],
        script_path,
        scratch_dir: scratch.clone(),
        read_only_dirs: system_python_dirs(),
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
    match envelope.get("line").and_then(Value::as_i64) {
        Some(line) => Err(KindError::InvalidSpec(format!(
            "source: syntax error at line {line}: {message}"
        ))),
        None => Err(KindError::InvalidSpec(format!("source: {message}"))),
    }
}

fn system_python_dirs() -> Vec<PathBuf> {
    ["/usr", "/lib", "/lib64", "/bin"]
        .into_iter()
        .map(PathBuf::from)
        .filter(|p| p.exists())
        .collect()
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

// ---- runner protocol envelope (this kind's own convention) ---------------

const PY_RUNNER_SCRIPT: &str = r#"
import sys, json, importlib.util, traceback

def emit(obj):
    sys.stdout.write(json.dumps(obj))
    sys.stdout.flush()

try:
    payload = json.loads(sys.stdin.read())
except Exception as e:
    emit({"ok": False, "kind": "exception", "error": "PayloadError", "message": str(e)})
    sys.exit(1)

site_packages = payload.get("site_packages")
if site_packages:
    sys.path.insert(0, site_packages)
args = payload.get("args", {})

try:
    spec = importlib.util.spec_from_file_location("tool", "tool.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    if not hasattr(module, "main"):
        emit({"ok": False, "kind": "exception", "error": "AttributeError", "message": "tool module has no main(args)", "traceback": ""})
        sys.exit(1)
    result = module.main(args)
except MemoryError:
    emit({"ok": False, "kind": "oom"})
    sys.exit(1)
except Exception as e:
    emit({"ok": False, "kind": "exception", "error": type(e).__name__, "message": str(e), "traceback": traceback.format_exc()})
    sys.exit(1)
else:
    try:
        emit({"ok": True, "result": result})
    except (TypeError, ValueError) as e:
        emit({"ok": False, "kind": "output_invalid", "message": str(e)})
        sys.exit(1)
"#;

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
            KindError::structured_with(
                "tool_exception",
                format!("{error}: {message}"),
                json!({"traceback": traceback, "stderr_tail": stderr_tail}),
            )
        }
    }
}

fn map_sandbox_outcome(outcome: SandboxOutcome) -> Result<Value, KindError> {
    match outcome {
        SandboxOutcome::Exited { stdout, .. } => match serde_json::from_slice::<Value>(&stdout) {
            Ok(envelope) if envelope.get("ok").and_then(Value::as_bool) == Some(true) => {
                Ok(envelope.get("result").cloned().unwrap_or(Value::Null))
            }
            Ok(envelope) => Err(map_envelope_error(&envelope, "")),
            Err(_) => Err(KindError::structured(
                "tool_output_invalid",
                "tool stdout was not valid JSON",
            )),
        },
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
}

impl PythonKind {
    /// Production constructor: `data_dir` is `$MCPHOST_DATA_DIR`; envs live
    /// under `<data_dir>/envs`, per-call scratch dirs under
    /// `<data_dir>/scratch`. Mechanism auto-detected (bwrap preferred).
    pub fn new(data_dir: &Path) -> Self {
        Self::build(
            data_dir,
            sandbox::detect_mechanism(),
            DEFAULT_MAX_CONCURRENT_CALLS,
            DEFAULT_CPU_BUDGET_MS_PER_HOUR,
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
        )
    }

    fn build(
        data_dir: &Path,
        isolation: IsolationMechanism,
        max_concurrent: usize,
        cpu_budget_ms: u64,
    ) -> Self {
        Self {
            envs_root: data_dir.join("envs"),
            scratch_root: data_dir.join("scratch"),
            isolation,
            envs: Arc::new(EnvRegistry::new()),
            semaphore: Arc::new(Semaphore::new(max_concurrent)),
            cpu_budget: CpuBudget::new(cpu_budget_ms),
        }
    }

    /// The active isolation mechanism, for `/healthz` (requirement 5).
    pub fn mechanism(&self) -> &'static str {
        self.isolation.as_str()
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

    async fn validate_async(&self, spec: &Value) -> Result<(), KindError> {
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

    async fn call(&self, spec: &Value, args: Value, ctx: &CallCtx) -> Result<Value, KindError> {
        let parsed = parse_spec(spec)?;
        validate_spec_fields(&parsed)?;

        let effective_schema = parsed.effective_args_schema()?;
        let validator = jsonschema::validator_for(&effective_schema)
            .map_err(|e| KindError::InvalidSpec(format!("args_schema: {e}")))?;
        if let Err(e) = validator.validate(&args) {
            return Err(KindError::structured_with(
                "args_invalid",
                e.to_string(),
                json!({"instance_path": e.instance_path.to_string()}),
            ));
        }

        let Ok(_permit) = self.semaphore.clone().try_acquire_owned() else {
            return Err(KindError::structured(
                "capacity",
                "at the concurrent-call limit; try again shortly",
            ));
        };

        let secret_env = self.secret_env(&parsed, ctx);

        if let Err(retry_after_s) = self.cpu_budget.check(ctx.tenant_id) {
            return Err(KindError::structured_with(
                "rate_limited",
                "tenant CPU budget exceeded for this hour",
                json!({"retry_after_s": retry_after_s}),
            ));
        }

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
        read_only_dirs.push(env_dir.clone());

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

        let (cpu_ms, peak_rss_kb) = outcome_usage(&outcome);
        ctx.resources.record(cpu_ms, peak_rss_kb);
        self.cpu_budget.record(ctx.tenant_id, cpu_ms);

        // Requirement 7: secret values are "redacted from any string that
        // leaves the host" -- that includes a tool's own result (a tool may
        // legitimately be handed a secret and choose to echo it back, e.g.
        // while debugging), not just an error/traceback.
        let secret_values: Vec<String> = secret_env.iter().map(|(_, v)| v.clone()).collect();
        match map_sandbox_outcome(outcome) {
            Ok(value) => {
                let redacted = redact_value(&value, &secret_values);
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

    fn example(&self) -> KindExample {
        KindExample {
            spec: json!({
                "source": "def main(args):\n    return {\"doubled\": args[\"n\"] * 2}\n",
            }),
            call_args: json!({"n": 3}),
            blurb: "only source is required -- args_schema and requirements are both \
                inferred from it (tool-infer, v0.4.0); source must define main(args).",
        }
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
            println!("skipped: no user namespaces");
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
            println!("skipped: no user namespaces");
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
}
