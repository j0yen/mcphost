//! The `js` kind (PRD-mcphost-js-kind): `spec.source` defines `main(args)`
//! (or `export default`), run by QuickJS inside the same bwrap profile the
//! `python` kind uses, with the same budgets (`timeout_s`, `memory_mb`,
//! `network`, `secrets`, `env`, `outputs`) and a `mcphost` object whose
//! members are generated from [`python::BRIDGE_MODULES`], so the two bridges
//! cannot drift.
//!
//! ## Engine form
//!
//! The engine is the `mcphost-qjs` binary (`src/bin/mcphost-qjs.rs`, the
//! `rquickjs` crate's bundled QuickJS): one subprocess per call, started by
//! [`crate::sandbox::spawn_persistent`] under the python profile, speaking
//! python's line protocol -- request line in, envelope line out, mid-call
//! `mcphost.*` marker lines answered by [`python::HostSidecarBridge`].
//! Subprocess is the default engine form because isolation is then
//! kernel-level; `jsk_ac01_cold_start_bench` measures the cold start
//! (`cold_p95_ms=<n>`) and fails the build above [`COLD_P95_BUDGET_MS`].
//!
//! The binary is found next to the running executable (`cargo` puts both
//! `mcphost` and `mcphost-qjs` in `target/<profile>/`), or at
//! `$MCPHOST_QJS`; deploys ship the two side by side.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{Value, json};

use super::python::{self, BRIDGE_MODULES};
use crate::sandbox::{self, IsolationMechanism, NetworkMode, ResourceLimits};

/// AC1's budget: a cold call's p95 over 50 runs, in milliseconds.
pub const COLD_P95_BUDGET_MS: u64 = 300;

/// The engine runner's file name.
pub const QJS_BINARY: &str = "mcphost-qjs";

/// The file the tool source is written to inside the sandbox; the engine
/// reports positions against this name (`at tool.js:3`).
pub const TOOL_FILE: &str = "tool.js";

/// Address-space headroom added on top of `memory_mb` for the runner binary
/// and libc themselves: QuickJS enforces `memory_mb` on the script's heap
/// (`--memory-mb`), `RLIMIT_AS` is the kernel backstop above it.
const AS_HEADROOM_MB: u64 = 64;

const MAX_OPEN_FILES: u64 = 64;
const MAX_FILE_SIZE_MB: u64 = 16;
const MAX_PROCESSES: u64 = 64;

/// Where the engine runner lives, or why it can't be found.
pub fn qjs_binary() -> Result<PathBuf, super::KindError> {
    if let Some(p) = std::env::var_os("MCPHOST_QJS").map(PathBuf::from)
        && p.is_file()
    {
        return Ok(p);
    }
    let exe = std::env::current_exe().ok();
    // `target/<profile>/mcphost` and, for an integration test binary,
    // `target/<profile>/deps/<test>`.
    for dir in exe.iter().flat_map(|e| e.ancestors().skip(1).take(2)) {
        let candidate = dir.join(QJS_BINARY);
        if candidate.is_file() {
            return Ok(candidate);
        }
    }
    Err(super::KindError::Exec(format!(
        "the js engine ({QJS_BINARY}) was not found next to the server binary; set MCPHOST_QJS"
    )))
}

/// `(function, [param names])` for one bridge signature such as
/// `mcphost.state.get(key, default=None) -- read a value`; `function` is
/// empty for a callable module (`mcphost.elicit(...)`).
fn parse_signature<'a>(module: &str, sig: &'a str) -> (&'a str, Vec<&'a str>) {
    let head = sig.split(" -- ").next().unwrap_or(sig);
    let (path, params) = head.split_once('(').unwrap_or((head, ""));
    let function = path
        .strip_prefix("mcphost.")
        .and_then(|p| p.strip_prefix(module))
        .map(|p| p.trim_start_matches('.'))
        .unwrap_or("");
    let params = params
        .trim_end_matches(')')
        .split(',')
        .map(|p| p.split('=').next().unwrap_or("").trim())
        .filter(|p| !p.is_empty())
        .collect();
    (function, params)
}

/// The bridge table the prelude is built from: `{module: {function:
/// [params]}}`, or `{module: [params]}` for a callable module. Generated
/// from [`BRIDGE_MODULES`] and nothing else.
fn bridge_table() -> Value {
    let mut table = serde_json::Map::new();
    for module in BRIDGE_MODULES {
        let mut functions = serde_json::Map::new();
        let mut callable: Option<Vec<&str>> = None;
        for sig in module.signatures {
            let (function, params) = parse_signature(module.name, sig);
            if function.is_empty() {
                callable = Some(params);
            } else {
                functions.insert(function.to_string(), json!(params));
            }
        }
        table.insert(
            module.name.to_string(),
            callable.map_or(Value::Object(functions), |p| json!(p)),
        );
    }
    Value::Object(table)
}

/// The JS prelude: defines the global `mcphost` object (one member per
/// [`BRIDGE_MODULES`] entry plus `call` and `progress` (the
/// [`python::BRIDGE_ATTRS`] this kind binds; `mcphost.quota` is not bound),
/// `process.env` and `fetch`. Only the bridge table and
/// the unknown-member hint are interpolated; the wire behavior below is the
/// one rule set python's runner implements (a marker line out, one response
/// line in).
pub fn prelude() -> String {
    PRELUDE_TEMPLATE
        .replace("__BRIDGE_TABLE__", &bridge_table().to_string())
        .replace("__BRIDGE_LIST__", &json!(python::bridge_module_list()).to_string())
}

const PRELUDE_TEMPLATE: &str = r#"(function (g) {
  const table = __BRIDGE_TABLE__;
  const available = __BRIDGE_LIST__;
  const send = (marker, payload) => {
    const resp = JSON.parse(__mcphost_bridge(JSON.stringify(Object.assign({ [marker]: true }, payload))));
    if (!resp.ok) {
      const e = new Error(resp.message || "bridge call failed");
      e.name = "McphostBridgeError";
      e.code = resp.code;
      e.data = resp.data || {};
      throw e;
    }
    return resp.result;
  };
  const named = (params, values) => {
    const out = {};
    params.forEach((p, i) => { if (values[i] !== undefined) out[p] = values[i]; });
    return out;
  };
  const b64chars = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
  const toBytes = (data) => {
    if (typeof data !== "string") return Array.from(data);
    const out = [];
    for (const ch of data) {
      const c = ch.codePointAt(0);
      if (c < 0x80) out.push(c);
      else if (c < 0x800) out.push(0xc0 | (c >> 6), 0x80 | (c & 63));
      else if (c < 0x10000) out.push(0xe0 | (c >> 12), 0x80 | ((c >> 6) & 63), 0x80 | (c & 63));
      else out.push(0xf0 | (c >> 18), 0x80 | ((c >> 12) & 63), 0x80 | ((c >> 6) & 63), 0x80 | (c & 63));
    }
    return out;
  };
  const encode = (bytes) => {
    let s = "";
    for (let i = 0; i < bytes.length; i += 3) {
      const n = (bytes[i] << 16) | ((bytes[i + 1] || 0) << 8) | (bytes[i + 2] || 0);
      s += b64chars[(n >> 18) & 63] + b64chars[(n >> 12) & 63]
        + (i + 1 < bytes.length ? b64chars[(n >> 6) & 63] : "=")
        + (i + 2 < bytes.length ? b64chars[n & 63] : "=");
    }
    return s;
  };
  const decode = (text) => {
    const out = [];
    let acc = 0, bits = 0;
    for (const ch of text.replace(/=+$/, "")) {
      acc = (acc << 6) | b64chars.indexOf(ch);
      bits += 6;
      if (bits >= 8) { bits -= 8; out.push((acc >> bits) & 255); }
    }
    return Uint8Array.from(out);
  };
  // Per-function wire shapes that differ from "marker = module, op = name".
  const special = {
    "state.get": (p, a) => {
      const r = send("__mcphost_state__", { op: "get", args: { key: a[0] } });
      return r && r.found ? r.value : (a[1] === undefined ? null : a[1]);
    },
    "docs.get": (p, a) => send("__mcphost_docs__", { op: "get", args: Object.assign({ text: true }, named(p, a)) }).text,
    "blob.get": (p, a) => decode(send("__mcphost_docs__", { op: "blob_get", args: { ref: a[0] } }).data_base64),
    "blob.put": (p, a) => send("__mcphost_docs__", {
      op: "blob_put",
      args: Object.assign({ name: a[0], data_base64: encode(toBytes(a[1])) }, a[2] === undefined ? {} : { mime: a[2] }),
    }),
  };
  const mod = {};
  for (const name of Object.keys(table)) {
    const spec = table[name];
    if (Array.isArray(spec)) {
      // A callable module: mcphost.elicit(message, schema, timeout_s).
      mod[name] = (...a) => send("__mcphost_" + name + "__", named(spec, a));
      continue;
    }
    const marker = name === "blob" ? "__mcphost_docs__" : "__mcphost_" + name + "__";
    const fns = {};
    for (const fn of Object.keys(spec)) {
      const params = spec[fn];
      const key = name + "." + fn;
      fns[fn] = (...a) => special[key]
        ? special[key](params, a)
        : send(marker, { op: fn, args: named(params, a) });
    }
    mod[name] = fns;
  }
  mod.call = (name, args, timeout_s) => {
    const req = { name, args: args || {} };
    if (timeout_s !== undefined) req.timeout_s = timeout_s;
    return send("__mcphost_call__", req);
  };
  mod.progress = (pct, msg) => {
    const resp = JSON.parse(__mcphost_bridge(JSON.stringify({
      __mcphost_progress__: true, pct: pct === undefined ? null : pct, msg: msg === undefined ? null : msg,
    })));
    return resp.ok;
  };
  g.mcphost = new Proxy(mod, {
    get(target, prop) {
      if (typeof prop === "symbol" || prop in target || prop === "then" || prop === "toJSON") return target[prop];
      throw new ReferenceError("mcphost." + prop + " is not a bridge; available: " + available + ", mcphost.call, mcphost.progress");
    },
  });
  const env = JSON.parse(g.__mcphost_env);
  const eu = JSON.parse(g.__mcphost_end_user);
  if (eu) {
    env.MCPHOST_END_USER_ID = eu.id || "";
    env.MCPHOST_END_USER_METHOD = eu.method || "";
    if (eu.issuer) env.MCPHOST_END_USER_ISSUER = eu.issuer;
    if (eu.email) env.MCPHOST_END_USER_EMAIL = eu.email;
    if (eu.name) env.MCPHOST_END_USER_NAME = eu.name;
  }
  g.process = { env };
  g.fetch = () => {
    throw new Error(g.__mcphost_network === "none"
      ? "fetch is blocked: this tool has no network grant (spec network: none); set network to \"egress\" to allow outbound requests"
      : "fetch is not available in the js kind; call an http tool with mcphost.call instead");
  };
})(globalThis);
"#;

/// Everything one sandboxed call is started with.
pub struct CallParams<'a> {
    pub source: &'a str,
    pub timeout_s: u64,
    pub memory_mb: u64,
    pub network: NetworkMode,
    pub extra_env: Vec<(String, String)>,
    pub isolation: IsolationMechanism,
}

/// Writes `tool.js` and the prelude into a fresh scratch dir under `root`.
pub async fn prepare_scratch(root: &Path, source: &str) -> std::io::Result<PathBuf> {
    let scratch = root.join(format!("jscall-{}-{}", std::process::id(), rand::random::<u64>()));
    tokio::fs::create_dir_all(&scratch).await?;
    tokio::fs::write(scratch.join(TOOL_FILE), source).await?;
    tokio::fs::write(scratch.join("prelude.js"), prelude()).await?;
    Ok(scratch)
}

/// The python bwrap profile around the engine runner: python's read-only
/// system dirs plus the runner binary, python's rlimits, the call's own
/// network grant and environment.
pub fn run_spec(params: &CallParams<'_>, scratch: &Path, qjs: &Path) -> sandbox::RunSpec {
    let mut read_only_dirs = python::system_python_dirs();
    read_only_dirs.push(qjs.to_path_buf());
    let network_flag = match params.network {
        NetworkMode::None => "none",
        NetworkMode::Public { .. } => "public",
    };
    sandbox::RunSpec {
        interpreter: qjs.to_path_buf(),
        interpreter_args: vec![
            "--memory-mb".to_string(),
            params.memory_mb.to_string(),
            "--timeout-ms".to_string(),
            (params.timeout_s * 1000).to_string(),
            "--network".to_string(),
            network_flag.to_string(),
            "--prelude".to_string(),
            scratch.join("prelude.js").to_string_lossy().into_owned(),
        ],
        script_path: scratch.join(TOOL_FILE),
        scratch_dir: scratch.to_path_buf(),
        read_only_dirs,
        stdin_payload: Vec::new(),
        limits: ResourceLimits {
            // The engine's own deadline (`--timeout-ms`) fires first; these
            // are the kernel backstops above it.
            cpu_seconds: params.timeout_s + 1,
            memory_mb: params.memory_mb + AS_HEADROOM_MB,
            max_open_files: MAX_OPEN_FILES,
            max_file_size_mb: MAX_FILE_SIZE_MB,
            max_processes: MAX_PROCESSES,
        },
        wall_clock_timeout: Duration::from_secs(params.timeout_s + 1),
        network: params.network.clone(),
        extra_env: params.extra_env.clone(),
        isolation: params.isolation,
    }
}

/// One cold call: scratch dir, spawn, one request line, one envelope line,
/// kill -- python's `run_cold_interactive` over the js runner.
pub async fn cold_call(
    scratch_root: &Path,
    params: &CallParams<'_>,
    payload: &Value,
    bridge: &dyn sandbox::SidecarBridge,
    cancel_pid: &Arc<crate::runs::CancelSlot>,
) -> Result<sandbox::SandboxOutcome, super::KindError> {
    let qjs = qjs_binary()?;
    let scratch = prepare_scratch(scratch_root, params.source)
        .await
        .map_err(|e| super::KindError::Exec(format!("scratch dir: {e}")))?;
    let spec = run_spec(params, &scratch, &qjs);
    let bytes = serde_json::to_vec(payload).unwrap_or_default();
    let outcome = python::run_cold_interactive(&spec, &bytes, bridge, cancel_pid).await;
    let _ = tokio::fs::remove_dir_all(&scratch).await;
    outcome.map_err(|e| super::KindError::Exec(format!("sandbox spawn failed: {e}")))
}

// ---- the spec -----------------------------------------------------------------

use std::collections::BTreeMap;
use std::sync::Arc;

use serde::Deserialize;
use tokio::sync::Semaphore;

use super::contract::{
    BudgetDim, COMPOSE_FAMILY, CONFORMANCE_SECRET_NAME, ConformanceFixtures, ErrorFamily, KindContract, SourcePosition, TOOL_LIMIT_FAMILY, TOOL_STATE_FAMILY, UPSTREAM_FAMILY, codes, key,
};
use super::secretenv::{environment_report, merge_env, redact_kind_error, redact_value, secret_env};
use super::{CallCtx, Kind, KindError, KindExample, OutputDecl, ToolDescriptor};
use crate::sandbox::SandboxOutcome;

const MAX_SOURCE_BYTES: usize = 64 * 1024;
/// `timeout_s` default (PRD R1: "defaults equal python's" -- 10, 256, none).
const DEFAULT_TIMEOUT_S: u64 = 10;
const DEFAULT_MEMORY_MB: u64 = 256;
/// The ceilings js shares with python.
const MAX_TIMEOUT_S: u64 = python::MAX_TIMEOUT_S;
const MAX_MEMORY_MB: u64 = 1024;
/// How long past `timeout_s` the host waits before its own `call_timeout`:
/// the engine's deadline (`timeout_s`) and the sandbox wall clock
/// (`timeout_s + 1`) both fire first, so a runaway tool is reported as
/// `tool_timeout`, never as the generic host deadline.
const HOST_DEADLINE_GRACE_S: u64 = 2;

#[derive(Debug, Clone)]
struct JsSpec {
    source: String,
    args_schema: Option<Value>,
    timeout_s: Option<u64>,
    memory_mb: Option<u64>,
    network: Option<String>,
    secrets: Vec<String>,
    env: BTreeMap<String, String>,
    description: Option<String>,
    outputs: Vec<OutputDecl>,
}

#[derive(Debug, Clone, Deserialize)]
struct JsSpecRaw {
    source: String,
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
    env: BTreeMap<String, String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default = "no_outputs")]
    outputs: Value,
}

fn no_outputs() -> Value {
    Value::Array(Vec::new())
}

fn js_field_hint(field: &str) -> Option<(&'static str, Value)> {
    Some(match field {
        "source" => ("a string", json!("function main(args) {\n  return { ok: true };\n}\n")),
        "args_schema" => ("a JSON Schema object", json!({"type": "object"})),
        "timeout_s" => ("a positive integer number of seconds", json!(10)),
        "memory_mb" => ("a positive integer number of megabytes", json!(256)),
        "network" => (crate::network_policy::network_field_description(), json!("none")),
        "secrets" => ("a list of strings", json!(["api_key"])),
        "env" => (
            "a map of name to string value; names must match ^[A-Z][A-Z0-9_]{0,63}$",
            json!({"MODE": "fast"}),
        ),
        "description" => ("a string", json!("Totals a list of items.")),
        "outputs" => (
            "a list of field names or an object mapping field names to path strings",
            json!({"total": "$.total"}),
        ),
        _ => return None,
    })
}

impl JsSpec {
    fn effective_args_schema(&self) -> Value {
        self.args_schema.clone().unwrap_or_else(|| infer_args_schema(&self.source))
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

fn parse_spec(spec: &Value) -> Result<JsSpec, KindError> {
    if !spec.is_object() {
        return Err(KindError::InvalidSpec("spec: must be a JSON object".into()));
    }
    let raw: JsSpecRaw = serde_path_to_error::deserialize(spec.clone())
        .map_err(|e| super::spec_parse_error(spec, e, js_field_hint))?;
    let outputs = super::normalize_outputs(&raw.outputs)?;
    Ok(JsSpec {
        source: raw.source,
        args_schema: raw.args_schema,
        timeout_s: raw.timeout_s,
        memory_mb: raw.memory_mb,
        network: raw.network,
        secrets: raw.secrets,
        env: raw.env,
        description: raw.description,
        outputs,
    })
}

// ---- args_schema inference (R6) ---------------------------------------------

/// `args.<name>` member reads, `main({a, b})` destructuring, and JSDoc
/// `@param {type} args.<name>` lines give the properties; an unreadable
/// source gives `{type: object}`. A pure function of `source`.
pub fn infer_args_schema(source: &str) -> Value {
    let mut props: BTreeMap<String, Value> = BTreeMap::new();
    let ident = |s: &str| -> String {
        s.chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '$').collect()
    };
    let mut rest = source;
    while let Some(i) = rest.find("args.") {
        let before = rest[..i].chars().last();
        let name = ident(&rest[i + 5..]);
        if !name.is_empty() && !before.is_some_and(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.') {
            props.entry(name).or_insert_with(|| json!({}));
        }
        rest = &rest[i + 5..];
    }
    if let Some(i) = source.find("main(").or_else(|| source.find("main ("))
        && let Some(open) = source[i..].find('{')
        && source[i..i + open].trim_end_matches(['(', ' ']).ends_with("main")
        && let Some(close) = source[i + open..].find('}')
    {
        for part in source[i + open + 1..i + open + close].split(',') {
            let name = ident(part.trim());
            if !name.is_empty() {
                props.entry(name).or_insert_with(|| json!({}));
            }
        }
    }
    for line in source.lines() {
        let Some(p) = line.find("@param") else { continue };
        let tail = line[p + 6..].trim_start();
        let Some(inner) = tail.strip_prefix('{').and_then(|t| t.split_once('}')) else { continue };
        let ty = match inner.0.trim() {
            "string" => "string",
            "number" => "number",
            "integer" => "integer",
            "boolean" => "boolean",
            "object" | "Object" => "object",
            "array" | "Array" => "array",
            t if t.ends_with("[]") => "array",
            _ => continue,
        };
        if let Some(name) = inner.1.trim_start().strip_prefix("args.") {
            props.insert(ident(name), json!({"type": ty}));
        }
    }
    if props.is_empty() {
        return json!({"type": "object"});
    }
    json!({"type": "object", "properties": props})
}

// ---- TypeScript refusal (R8) --------------------------------------------------

/// The first line holding a TypeScript-only construct (a parameter, variable
/// or return type annotation, `interface`, `type X =`), as `(line, text)`.
fn typescript_construct(source: &str) -> Option<(usize, String)> {
    let is_ident = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '$');
    for (i, raw) in source.lines().enumerate() {
        let line = raw.trim();
        if line.starts_with("//") || line.starts_with('*') || line.starts_with("/*") {
            continue;
        }
        let ts = line.starts_with("interface ")
            || line.starts_with("export interface ")
            || (line.starts_with("type ") && line.contains(" = "))
            || (line.starts_with("export type ") && line.contains(" = "))
            || annotated_binding(line)
            || annotated_params(line, &is_ident);
        if ts {
            return Some((i + 1, line.to_string()));
        }
    }
    None
}

fn annotated_binding(line: &str) -> bool {
    ["const ", "let ", "var "].iter().any(|kw| {
        line.strip_prefix(kw).is_some_and(|rest| {
            let name: String = rest.chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '$').collect();
            !name.is_empty() && rest[name.len()..].trim_start().starts_with(':')
        })
    })
}

fn annotated_params(line: &str, is_ident: &dyn Fn(&str) -> bool) -> bool {
    let Some(open) = line.find("function").and_then(|f| line[f..].find('(').map(|p| f + p)) else {
        return false;
    };
    let Some(close) = line[open..].find(')').map(|p| open + p) else { return false };
    let params = &line[open + 1..close];
    let annotated = params.split(',').any(|p| {
        let p = p.trim();
        p.split_once(':').is_some_and(|(n, t)| is_ident(n.trim().trim_end_matches('?')) && !t.trim().is_empty())
    });
    let returns = line[close + 1..].trim_start().strip_prefix(':').is_some_and(|t| !t.trim().is_empty() && !t.trim_start().starts_with('{') || t.contains("=>"));
    annotated || returns
}

fn typescript_error(line: usize, text: &str) -> KindError {
    KindError::structured_with(
        TYPESCRIPT_NOT_SUPPORTED,
        format!("source: line {line} uses TypeScript syntax; the js kind runs plain JavaScript (ES2020)"),
        json!({
            "field": "source",
            "line": line,
            "source_line": text,
            "hint": "strip the types first, e.g. `npx tsc --target es2020 --module es2020 tool.ts`, and publish the emitted tool.js",
        }),
    )
}

// ---- validation ---------------------------------------------------------------

fn validate_fields_all(parsed: &JsSpec) -> Vec<KindError> {
    let mut errors = Vec::new();
    if parsed.source.is_empty() {
        errors.push(KindError::InvalidSpec("source: must not be empty".into()));
    } else if parsed.source.len() > MAX_SOURCE_BYTES {
        errors.push(KindError::InvalidSpec(format!(
            "source: {} bytes, over the {MAX_SOURCE_BYTES}-byte limit",
            parsed.source.len()
        )));
    } else if let Some((line, text)) = typescript_construct(&parsed.source) {
        errors.push(typescript_error(line, &text));
    }
    errors.extend(python::validate_env_all(&parsed.env));
    if let Some(schema) = &parsed.args_schema
        && let Err(e) = jsonschema::validator_for(schema)
    {
        errors.push(KindError::InvalidSpec(format!("args_schema: not a valid JSON Schema: {e}")));
    }
    if let Some(t) = parsed.timeout_s
        && !(1..=MAX_TIMEOUT_S).contains(&t)
    {
        errors.push(KindError::InvalidSpec(format!("timeout_s: must be between 1 and {MAX_TIMEOUT_S}; got {t}")));
    }
    if let Some(m) = parsed.memory_mb
        && !(1..=MAX_MEMORY_MB).contains(&m)
    {
        errors.push(KindError::InvalidSpec(format!("memory_mb: must be between 1 and {MAX_MEMORY_MB}; got {m}")));
    }
    if let Some(n) = &parsed.network
        && n != "none"
        && n != "public"
        && n != "egress"
    {
        errors.push(KindError::InvalidSpec(format!("network: must be 'none', 'public', or 'egress'; got '{n}'")));
    }
    errors
}

/// Parses `source` with the engine (`mcphost-qjs --check`) inside the same
/// sandbox profile a call runs in -- never in the host process.
async fn engine_check(scratch_root: &Path, source: &str, isolation: IsolationMechanism) -> Result<(), KindError> {
    let qjs = qjs_binary()?;
    let scratch = scratch_root.join(format!("jscheck-{}-{}", std::process::id(), rand::random::<u64>()));
    tokio::fs::create_dir_all(&scratch)
        .await
        .map_err(|e| KindError::Exec(format!("check scratch dir: {e}")))?;
    tokio::fs::write(scratch.join(TOOL_FILE), source)
        .await
        .map_err(|e| KindError::Exec(format!("write tool source: {e}")))?;
    let mut spec = run_spec(
        &CallParams {
            source,
            timeout_s: 5,
            memory_mb: 128,
            network: NetworkMode::None,
            extra_env: Vec::new(),
            isolation,
        },
        &scratch,
        &qjs,
    );
    spec.interpreter_args = vec!["--check".to_string()];
    let outcome = sandbox::run(spec).await;
    let _ = tokio::fs::remove_dir_all(&scratch).await;
    let outcome = outcome.map_err(|e| KindError::Exec(format!("js check spawn: {e}")))?;
    let SandboxOutcome::Exited { stdout, .. } = outcome else {
        return Err(KindError::Exec(format!("js check did not complete cleanly: {outcome:?}")));
    };
    let envelope: Value = serde_json::from_slice(&stdout)
        .map_err(|e| KindError::Exec(format!("js check produced non-JSON output: {e}")))?;
    if envelope.get("ok").and_then(Value::as_bool) == Some(true) {
        return Ok(());
    }
    let message = envelope.get("message").and_then(Value::as_str).unwrap_or("source is not a valid js tool");
    let class = envelope.get("error").and_then(Value::as_str).unwrap_or("InvalidSpec");
    let mut data = json!({"field": "source", "exception_class": class});
    let mut text = format!("source: {message}");
    if let Some(line) = envelope.get("line").and_then(Value::as_i64) {
        text = format!("source: syntax error at line {line}: {message}");
        data["line"] = json!(line);
        for k in ["column", "source_line"] {
            if let Some(v) = envelope.get(k) {
                data[k] = v.clone();
            }
        }
    }
    Err(KindError::structured_with(crate::errors::INVALID_SPEC, text, data))
}

// ---- envelope -> result / error ---------------------------------------------------

/// `(line, column)` of the first `tool.js:L[:C]` frame in an engine stack
/// (`    at main (tool.js:3:13)`), the shape python's
/// `extract_tool_source_line` returns for a traceback.
pub fn extract_tool_source_line(stack: &str) -> Option<(i64, Option<i64>)> {
    stack.lines().find_map(|frame| {
        let at = frame.find("tool.js:")?;
        let mut nums = frame[at + TOOL_FILE.len() + 1..].split(|c: char| !c.is_ascii_digit());
        let line = nums.next()?.parse().ok()?;
        Some((line, nums.next().and_then(|c| c.parse().ok())))
    })
}

fn map_envelope_error(envelope: &Value, stderr_tail: &str) -> KindError {
    super::contract::tag_families(map_envelope_error_untagged(envelope, stderr_tail))
}

fn map_envelope_error_untagged(envelope: &Value, stderr_tail: &str) -> KindError {
    let text = |k: &str| envelope.get(k).and_then(Value::as_str).unwrap_or("");
    match text("kind") {
        "oom" => KindError::structured(codes::TOOL_OOM, "the call exceeded its memory limit"),
        "timeout" => KindError::structured(codes::TOOL_TIMEOUT, "the call exceeded its timeout"),
        "output_invalid" => KindError::structured_with(
            codes::TOOL_OUTPUT_INVALID,
            if text("message").is_empty() { "tool result is not JSON-serializable" } else { text("message") },
            json!({"stderr_tail": stderr_tail}),
        ),
        _ => {
            let class = if text("error").is_empty() { "Error" } else { text("error") };
            let located = extract_tool_source_line(text("traceback"));
            let suffix = located.map(|(n, _)| format!(" at tool.js:{n}")).unwrap_or_default();
            let mut data = json!({
                "phase": key::PHASE_TOOL_CODE,
                "exception_class": class,
                "traceback": text("traceback"),
                "stderr_tail": stderr_tail,
            });
            if let Some((line, col)) = located {
                data["line"] = json!(line);
                if let Some(c) = col {
                    data["column"] = json!(c);
                }
                if let Some(src) = envelope.get("source_line") {
                    data["source_line"] = src.clone();
                }
            }
            KindError::structured_with(codes::TOOL_EXCEPTION, format!("{class}: {}{suffix}", text("message")), data)
        }
    }
}

fn map_envelope_line(line: &[u8], allow_oversized: bool) -> Result<Value, KindError> {
    if !allow_oversized && line.len() > crate::state::MAX_TOOL_OUTPUT_BYTES {
        return Err(KindError::structured_with(
            codes::TOOL_OUTPUT_TOO_LARGE,
            format!("tool output is {} bytes, over the {}-byte limit", line.len(), crate::state::MAX_TOOL_OUTPUT_BYTES),
            json!({"limit_bytes": crate::state::MAX_TOOL_OUTPUT_BYTES, "actual_bytes": line.len()}),
        ));
    }
    match serde_json::from_slice::<Value>(line) {
        Ok(envelope) => {
            crate::calltrace::note_envelope(&envelope);
            if envelope.get("ok").and_then(Value::as_bool) == Some(true) {
                Ok(envelope.get("result").cloned().unwrap_or(Value::Null))
            } else {
                Err(map_envelope_error(&envelope, ""))
            }
        }
        Err(_) => Err(KindError::structured(codes::TOOL_OUTPUT_INVALID, "tool stdout was not valid JSON")),
    }
}

fn map_outcome(outcome: SandboxOutcome, allow_oversized: bool) -> Result<Value, KindError> {
    match outcome {
        SandboxOutcome::Exited { stdout, .. } => map_envelope_line(&stdout, allow_oversized),
        SandboxOutcome::NonZeroExit { stdout_tail, stderr_tail, .. } => {
            crate::calltrace::note_process_tails(&stdout_tail, &stderr_tail);
            match serde_json::from_str::<Value>(&stdout_tail) {
                Ok(envelope) => Err(map_envelope_error(&envelope, &stderr_tail)),
                Err(_) => Err(KindError::structured_with(
                    codes::TOOL_EXCEPTION,
                    "the tool process exited with an error and produced no readable envelope",
                    json!({"stdout_tail": stdout_tail, "stderr_tail": stderr_tail}),
                )),
            }
        }
        // SIGABRT is the engine's allocator giving up at the RLIMIT_AS
        // backstop above `memory_mb`; SIGXCPU is RLIMIT_CPU past the deadline.
        SandboxOutcome::Signaled { signal: 6, .. } => {
            Err(KindError::structured(codes::TOOL_OOM, "the call exceeded its memory limit"))
        }
        SandboxOutcome::Signaled { signal: 24, .. } => {
            Err(KindError::structured(codes::TOOL_TIMEOUT, "the call exceeded its timeout"))
        }
        SandboxOutcome::Signaled { signal, stdout_tail, stderr_tail, .. } => {
            crate::calltrace::note_process_tails(&stdout_tail, &stderr_tail);
            Err(KindError::structured_with(
                codes::TOOL_EXCEPTION,
                format!("the tool process was killed by signal {signal}"),
                json!({"stdout_tail": stdout_tail, "stderr_tail": stderr_tail}),
            ))
        }
        SandboxOutcome::TimedOut { .. } => Err(KindError::structured(codes::TOOL_TIMEOUT, "the call exceeded its timeout")),
    }
}

// ---- the kind -------------------------------------------------------------------------

fn conformance_good_spec() -> Value {
    json!({
        "source": "function main(args) {\n  return { n: args.n * 2 };\n}\n",
        "args_schema": {"type": "object", "properties": {"n": {"type": "integer"}}, "required": ["n"]},
    })
}

/// Throws on line 3 of the tool source.
fn conformance_erroring_spec() -> Value {
    json!({"source": "function main(args) {\n  const x = 1;\n  throw new Error(\"boom\");\n}\n"})
}

fn conformance_slow_spec() -> Value {
    json!({"source": "function main(args) {\n  while (true) {}\n}\n"})
}

fn conformance_secret_spec() -> Value {
    json!({
        "source": format!(
            "function main(args) {{\n  return {{ v: process.env.SECRET_{} }};\n}}\n",
            CONFORMANCE_SECRET_NAME.to_ascii_uppercase()
        ),
        "secrets": [CONFORMANCE_SECRET_NAME],
    })
}

const JS_TOOL_CODE: ErrorFamily = ErrorFamily {
    name: super::contract::family::TOOL_CODE,
    codes: &[codes::TOOL_EXCEPTION, codes::TOOL_OUTPUT_INVALID],
};

static CONTRACT: KindContract = KindContract {
    error_families: &[JS_TOOL_CODE, TOOL_LIMIT_FAMILY, UPSTREAM_FAMILY, COMPOSE_FAMILY, TOOL_STATE_FAMILY],
    budgets: &[
        BudgetDim { field: "timeout_s", min: 1, max: MAX_TIMEOUT_S, default: DEFAULT_TIMEOUT_S },
        BudgetDim { field: "memory_mb", min: 1, max: MAX_MEMORY_MB, default: DEFAULT_MEMORY_MB },
    ],
    source_position: SourcePosition::Line,
    bridges: &["state", "table", "docs", "lineage", "channel", "msg"],
    secret_redaction: true,
    fixtures: ConformanceFixtures {
        good_spec: conformance_good_spec,
        good_args: || json!({"n": 3}),
        bad_args: || json!({"n": "not-a-number"}),
        erroring_spec: Some(conformance_erroring_spec),
        slow_spec: Some(conformance_slow_spec),
        secret_spec: Some(conformance_secret_spec),
        skips: &[],
    },
};

/// The `js` kind. One QuickJS subprocess per call, under python's bwrap
/// profile; the same admission ceiling as python's host-wide cap.
pub struct JsKind {
    scratch_root: PathBuf,
    isolation: IsolationMechanism,
    semaphore: Arc<Semaphore>,
    max_concurrent: usize,
}

impl JsKind {
    /// Production constructor: per-call scratch dirs live under
    /// `<data_dir>/js-scratch`; isolation mechanism auto-detected.
    pub fn new(data_dir: &Path) -> Self {
        Self::build(data_dir, sandbox::detect_mechanism(), python::DEFAULT_MAX_CONCURRENT_CALLS)
    }

    /// Test constructor: a small concurrency ceiling.
    pub fn for_test_with_concurrency(data_dir: &Path, max_concurrent: usize) -> Self {
        Self::build(data_dir, sandbox::detect_mechanism(), max_concurrent)
    }

    fn build(data_dir: &Path, isolation: IsolationMechanism, max_concurrent: usize) -> Self {
        let scratch_root = data_dir.join("js-scratch");
        let _ = std::fs::create_dir_all(&scratch_root);
        Self { scratch_root, isolation, semaphore: Arc::new(Semaphore::new(max_concurrent)), max_concurrent }
    }
}

#[async_trait::async_trait]
impl Kind for JsKind {
    fn name(&self) -> &'static str {
        "js"
    }

    fn validate(&self, spec: &Value) -> Result<(), KindError> {
        let parsed = parse_spec(spec)?;
        validate_fields_all(&parsed).into_iter().next().map_or(Ok(()), Err)
    }

    fn known_spec_fields(&self) -> &'static [&'static str] {
        &[
            "source",
            "args_schema",
            "timeout_s",
            "memory_mb",
            "network",
            "secrets",
            "env",
            "outputs",
            "description",
        ]
    }

    fn required_spec_field(&self) -> Option<&'static str> {
        Some("source")
    }

    fn spec_field_description(&self, field: &str) -> Option<String> {
        js_field_hint(field).map(|(hint, _)| hint.to_string())
    }

    fn validate_all(&self, spec: &Value) -> Vec<KindError> {
        match parse_spec(spec) {
            Ok(parsed) => validate_fields_all(&parsed),
            Err(e) => vec![e],
        }
    }

    async fn validate_async(&self, spec: &Value) -> Result<(), KindError> {
        let parsed = parse_spec(spec)?;
        engine_check(&self.scratch_root, &parsed.source, self.isolation).await
    }

    fn describe(&self, spec: &Value) -> ToolDescriptor {
        match parse_spec(spec) {
            Ok(parsed) => ToolDescriptor {
                name: "js".to_string(),
                description: parsed
                    .description
                    .clone()
                    .unwrap_or_else(|| "Runs a tenant-published JavaScript function.".to_string()),
                input_schema: parsed.effective_args_schema(),
            },
            Err(e) => ToolDescriptor {
                name: "js".to_string(),
                description: format!("invalid js tool spec: {e}"),
                input_schema: json!({"type": "object"}),
            },
        }
    }

    fn referenced_secrets(&self, spec: &Value) -> Vec<String> {
        parse_spec(spec).map(|p| p.secrets).unwrap_or_default()
    }

    fn env_map(&self, spec: &Value) -> BTreeMap<String, String> {
        parse_spec(spec).map(|p| p.env).unwrap_or_default()
    }

    fn redacted_spec_for_sharing(&self, spec: &Value) -> Value {
        let mut out = serde_json::Map::new();
        for k in ["source", "args_schema", "timeout_s", "network"] {
            if let Some(v) = spec.get(k) {
                out.insert(k.to_string(), v.clone());
            }
        }
        Value::Object(out)
    }

    /// The host deadline sits [`HOST_DEADLINE_GRACE_S`] past `timeout_s`, so
    /// the engine's own `tool_timeout` is what a runaway tool reports.
    fn requested_timeout(&self, spec: &Value) -> Option<Duration> {
        let parsed = parse_spec(spec).ok()?;
        let t = parsed.effective_timeout_s().min(MAX_TIMEOUT_S);
        Some(Duration::from_secs(t + HOST_DEADLINE_GRACE_S))
    }

    fn declared_outputs(&self, spec: &Value) -> Vec<OutputDecl> {
        parse_spec(spec).map(|p| p.outputs).unwrap_or_default()
    }

    fn payload_from_call_result<'a>(&self, call_result: &'a Value) -> Option<&'a Value> {
        call_result
            .get("payload")
            .or_else(|| call_result.get("result").and_then(|r| r.get("payload")))
    }

    fn example(&self) -> KindExample {
        super::docs::parse_kind_doc(include_str!("../../docs/kinds/js.md"))
    }

    fn field_docs(&self) -> &'static [super::kinddoc::FieldDoc] {
        use super::kinddoc::FieldDoc;
        &[
            FieldDoc { name: "source", ty: "string", meaning: "JavaScript source defining `main(args)` (or `export default`)" },
            FieldDoc { name: "args_schema", ty: "object", meaning: "JSON Schema for the call arguments, inferred from `source` when omitted" },
            FieldDoc { name: "timeout_s", ty: "integer", meaning: "seconds the sandboxed call may run" },
            FieldDoc { name: "memory_mb", ty: "integer", meaning: "megabytes the sandbox may use" },
            FieldDoc { name: "network", ty: "string", meaning: "`none`, `public` or `egress`: what the process may reach beyond the `mcphost.*` bridges" },
            FieldDoc { name: "secrets", ty: "array of string", meaning: "`host.secret_set` names injected as `SECRET_<NAME>` environment variables (`process.env`)" },
            FieldDoc { name: "env", ty: "object", meaning: "plain, non-secret environment variables (`process.env`)" },
            FieldDoc { name: "description", ty: "string", meaning: "human-readable text shown in `tools/list`" },
            FieldDoc { name: "outputs", ty: "array or object", meaning: "fields a caller can rely on in `result.payload`, optionally with a `$.a.b` path each" },
        ]
    }

    fn ceiling_fields(&self) -> &'static [&'static str] {
        &["budget.max_tool_latency_ms", "budget.max_wall_ms"]
    }

    fn bridge_modules(&self) -> Option<&'static [python::BridgeModule]> {
        Some(BRIDGE_MODULES)
    }

    fn contract(&self) -> &'static KindContract {
        &CONTRACT
    }

    async fn call(&self, spec: &Value, args: Value, ctx: &CallCtx) -> Result<Value, KindError> {
        let parsed = parse_spec(spec)?;
        if let Some(e) = validate_fields_all(&parsed).into_iter().next() {
            return Err(e);
        }
        let network = python::sandbox_network_mode(parsed.effective_network(), ctx.egress_allowed)?;

        let effective_schema = parsed.effective_args_schema();
        let validator = jsonschema::validator_for(&effective_schema)
            .map_err(|e| KindError::InvalidSpec(format!("args_schema: {e}")))?;
        if let Err(e) = validator.validate(&args) {
            let args_err = super::describe_args_error(&e);
            return Err(KindError::structured_with(crate::errors::ARGS_INVALID, args_err.message(), args_err.data()));
        }

        let Ok(_permit) = self.semaphore.clone().try_acquire_owned() else {
            let in_flight = self.max_concurrent.saturating_sub(self.semaphore.available_permits());
            return Err(python::host_capacity_error(in_flight, self.max_concurrent, 250));
        };

        let secret_env = secret_env(&parsed.secrets, ctx.secrets.as_ref());
        let secret_values: Vec<String> = secret_env.iter().map(|(_, v)| v.clone()).collect();
        let plain_env: Vec<(String, String)> = parsed.env.iter().map(|(k, v)| (k.clone(), v.clone())).collect();

        let params = CallParams {
            source: &parsed.source,
            timeout_s: parsed.effective_timeout_s(),
            memory_mb: parsed.effective_memory_mb(),
            network,
            extra_env: merge_env(&secret_env, &plain_env),
            isolation: self.isolation,
        };
        let payload = json!({"args": args, "end_user": python::payload_end_user(ctx)});
        let bridge = python::HostSidecarBridge { ctx };
        let outcome = cold_call(&self.scratch_root, &params, &payload, &bridge, &ctx.cancel_pid).await?;

        let (cpu_ms, peak_rss_kb) = python::outcome_usage(&outcome);
        ctx.resources.record(cpu_ms, peak_rss_kb);

        match map_outcome(outcome, ctx.run_id.is_some()) {
            Ok(value) => {
                let redacted = redact_value(&value, &secret_values);
                let redacted = super::apply_declared_outputs(redacted, &parsed.outputs);
                if ctx.test_mode {
                    Ok(json!({
                        "result": redacted,
                        "schema": effective_schema,
                        "environment": environment_report(&parsed.env, &parsed.secrets),
                    }))
                } else {
                    Ok(redacted)
                }
            }
            Err(e) => Err(redact_kind_error(e, &secret_values)),
        }
    }
}

crate::err_code! {
    pub TYPESCRIPT_NOT_SUPPORTED,
    code: "typescript_not_supported", family: Spec, class: AgentFault, retryable: false,
    remedy: "Strip the types and publish plain JavaScript (ES2020); data.line is the first line with TypeScript syntax.",
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn args_schema_is_inferred_from_member_reads_destructuring_and_jsdoc() {
        let schema = infer_args_schema("function main(args) {\n  return args.items.length + args.limit;\n}\n");
        assert_eq!(schema, json!({"type": "object", "properties": {"items": {}, "limit": {}}}));
        let schema = infer_args_schema("function main({ a, b }) { return a + b; }");
        assert_eq!(schema, json!({"type": "object", "properties": {"a": {}, "b": {}}}));
        let schema = infer_args_schema("/**\n * @param {string} args.name\n * @param {number[]} args.sizes\n */\nfunction main(args) { return args.name; }");
        assert_eq!(schema["properties"]["name"], json!({"type": "string"}));
        assert_eq!(schema["properties"]["sizes"], json!({"type": "array"}));
        assert_eq!(infer_args_schema("function main() { return 1; }"), json!({"type": "object"}));
    }

    #[test]
    fn typescript_annotations_are_found_and_plain_javascript_is_not() {
        let ts = "function main(args: { n: number }): number {\n  return args.n;\n}\n";
        assert_eq!(typescript_construct(ts).map(|(l, _)| l), Some(1));
        assert!(typescript_construct("interface Args { n: number }\nfunction main(args) { return 1; }").is_some());
        assert!(typescript_construct("function main(args) {\n  const x: number = 1;\n  return x;\n}").is_some());
        let js = "// a: b\nfunction main(args) {\n  const o = { a: 1 };\n  return cond ? o.a : 2;\n}\n";
        assert_eq!(typescript_construct(js), None);
        let KindError::Structured { code, data, .. } = typescript_error(1, "function main(args: any) {") else {
            panic!("structured");
        };
        assert_eq!(code, "typescript_not_supported");
        assert!(data["hint"].as_str().unwrap().contains("tsc --target es2020"));
        assert_eq!(data["line"], json!(1));
    }

    #[test]
    fn the_first_tool_frame_of_an_engine_stack_gives_line_and_column() {
        let stack = "    at helper (tool.js:7:3)\n    at main (tool.js:3:12)\n    at <eval> (eval_script:5:1)\n";
        assert_eq!(extract_tool_source_line(stack), Some((7, Some(3))));
        assert_eq!(extract_tool_source_line("    at <eval> (eval_script:5:1)\n"), None);
    }

    #[test]
    fn every_bridge_signature_has_a_binding_in_the_generated_table() {
        let table = bridge_table();
        for module in BRIDGE_MODULES {
            let entry = &table[module.name];
            for sig in module.signatures {
                let (function, params) = parse_signature(module.name, sig);
                if function.is_empty() {
                    assert_eq!(entry, &json!(params), "{sig}");
                } else {
                    assert_eq!(entry[function], json!(params), "{sig}");
                }
            }
        }
        let prelude = prelude();
        assert!(!prelude.contains("__BRIDGE_"), "unreplaced placeholder");
        assert!(prelude.contains("mcphost.sqlite") || prelude.contains("is not a bridge"));
    }
}
