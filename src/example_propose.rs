//! PRD-mcphost-example-from-published-tool: `host.example.propose` turns a
//! tenant's working tool plus a proof call into a rendered, validated,
//! admin-reviewable `examples/<name>/` directory.
//!
//! The pipeline is pure up to the proof call (which the handler runs through
//! `host.tool_call` under the tenant's own budget): [`parse`] reads the
//! arguments, [`Prepared::build`] snapshots the tool spec and renders the
//! files, [`render_files`] writes the directory in memory and validates it
//! with the same code `mcphost examples check` runs, [`pack_tar`] stores it.
//! Nothing here touches the filesystem.

use std::collections::BTreeMap;

use serde_json::{Value, json};

use crate::db::{ExampleProposalRow, ToolRow};
use crate::errors::AppError;
use crate::state::AppState;

/// R6: open (pending or approved) proposals one tenant may hold.
pub const MAX_OPEN_PROPOSALS: i64 = 3;
/// The rendered tar's ceiling; a larger rendering is `too_large`.
pub const TAR_MAX_BYTES: usize = 256 * 1024;
/// How long the proof call waits for a python environment to finish building.
pub const PROOF_BUILD_WAIT_MS: u64 = 20_000;
/// Cap on the result excerpt a `proof_failed` error carries.
pub const EXCERPT_MAX_CHARS: usize = 500;
/// R2: the license stated in `[provenance]` and in the tool description.
pub const LICENSE: &str = "MIT";

pub const PROPOSE_DESCRIPTION: &str = "Turn a working tool into a PR-ready example: snapshot the tool at its \
     current version, run proof_call {args, expect} against it now, render an example directory \
     (example.toml with [provenance], tools/, proof.sh) and store it as a proposal for admin \
     review. Refuses secret_leak {line} when the source holds a stored secret's literal value, \
     not_portable when it uses tenant state, tables, groups or another tenant's tool, proof_failed, \
     example_exists, too_many_proposals (3 open). Accepted examples carry license MIT. \
     Returns {proposal_id, status: pending, rendered}.";

/// Which `{{ secret.NAME }}` references and spec `secrets` a tool needs.
pub fn secrets_needed(spec: &Value) -> Vec<String> {
    let mut names = std::collections::BTreeSet::new();
    if let Some(list) = spec.get("secrets").and_then(Value::as_array) {
        names.extend(list.iter().filter_map(Value::as_str).map(str::to_string));
    }
    collect_secret_refs(spec, &mut names);
    names.into_iter().collect()
}

fn collect_secret_refs(v: &Value, out: &mut std::collections::BTreeSet<String>) {
    match v {
        Value::String(s) => scan_secret_refs(s, out),
        Value::Array(a) => a.iter().for_each(|x| collect_secret_refs(x, out)),
        Value::Object(m) => m.values().for_each(|x| collect_secret_refs(x, out)),
        _ => {}
    }
}

/// `{{ secret.NAME }}` / `{{secret:NAME}}` references in one string.
fn scan_secret_refs(s: &str, out: &mut std::collections::BTreeSet<String>) {
    let mut rest = s;
    while let Some(at) = rest.find("{{") {
        rest = &rest[at + 2..];
        let inner = rest.trim_start();
        let Some(after) = inner.strip_prefix("secret") else { continue };
        let Some(after) = after.strip_prefix('.').or_else(|| after.strip_prefix(':')) else { continue };
        let name: String = after.chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-').collect();
        if !name.is_empty() && after[name.len()..].trim_start().starts_with("}}") {
            out.insert(name);
        }
    }
}

/// The parsed `host.example.propose` arguments.
pub struct Request {
    pub tool: String,
    pub proof_args: Value,
    pub expect: String,
    pub title: String,
    pub summary: Option<String>,
    pub use_case: Vec<String>,
    pub clients: Vec<String>,
}

fn string_list(args: &Value, field: &str) -> Result<Option<Vec<String>>, AppError> {
    match args.get(field) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Array(a)) => a
            .iter()
            .map(|v| v.as_str().map(str::to_string).ok_or_else(|| AppError::InvalidArgs(format!("{field}: every entry must be a string"))))
            .collect::<Result<Vec<_>, _>>()
            .map(Some),
        Some(_) => Err(AppError::InvalidArgs(format!("{field}: must be an array of strings"))),
    }
}

pub fn parse(args: &Value) -> Result<Request, AppError> {
    let req_str = |field: &str| {
        args.get(field)
            .and_then(Value::as_str)
            .filter(|s| !s.trim().is_empty())
            .map(str::to_string)
            .ok_or_else(|| AppError::InvalidArgs(format!("missing required argument '{field}'")))
    };
    let proof = args
        .get("proof_call")
        .filter(|p| p.is_object())
        .ok_or_else(|| AppError::InvalidArgs("missing required argument 'proof_call' ({args, expect})".into()))?;
    let expect = proof
        .get("expect")
        .and_then(Value::as_str)
        .ok_or_else(|| AppError::InvalidArgs("proof_call.expect is required".into()))?
        .to_string();
    let proof_args = match proof.get("args") {
        None | Some(Value::Null) => json!({}),
        Some(a) if a.is_object() => a.clone(),
        Some(_) => return Err(AppError::InvalidArgs("proof_call.args must be an object".into())),
    };
    let use_case = string_list(args, "use_case")?
        .filter(|l| !l.is_empty())
        .ok_or_else(|| AppError::InvalidArgs("missing required argument 'use_case' (a non-empty list)".into()))?;
    Ok(Request {
        tool: req_str("tool")?,
        proof_args,
        expect,
        title: req_str("title")?,
        summary: args.get("summary").and_then(Value::as_str).map(str::to_string),
        use_case,
        clients: string_list(args, "clients")?.filter(|l| !l.is_empty()).unwrap_or_else(|| vec!["any".to_string()]),
    })
}

/// The example directory name a tool proposes: the tool name, with `_`
/// as `-` (the existing examples are hyphenated).
pub fn example_name(tool: &str) -> String {
    tool.replace('_', "-")
}

/// R5/AC6: is `name` (either spelling) already in the embedded index?
pub fn exists_in_index(tool: &str) -> bool {
    let hyphen = example_name(tool);
    crate::examples::EXAMPLES.iter().any(|e| e.name == hyphen || e.name == tool)
}

pub fn example_exists(tool: &str) -> AppError {
    AppError::Structured {
        code: EXAMPLE_EXISTS,
        message: format!("an example named `{}` is already in the gallery; rename the tool and propose again", example_name(tool)),
        data: json!({"name": example_name(tool)}),
    }
}

pub fn too_many_proposals(open: i64) -> AppError {
    AppError::Structured {
        code: TOO_MANY_PROPOSALS,
        message: format!("{open} proposals are already open (limit {MAX_OPEN_PROPOSALS}); wait for a decision"),
        data: json!({"open": open, "limit": MAX_OPEN_PROPOSALS}),
    }
}

/// The tool, snapshotted at its current version, with its rendered files.
pub struct Prepared {
    pub tool: ToolRow,
    pub name: String,
    pub secrets_needed: Vec<String>,
    /// Spec with host-injected `_`-prefixed keys removed.
    pub spec: Value,
}

impl Prepared {
    pub fn build(row: ToolRow) -> Prepared {
        let mut spec = row.spec.clone();
        if let Some(obj) = spec.as_object_mut() {
            obj.retain(|k, _| !k.starts_with('_'));
        }
        Prepared { name: example_name(&row.name), secrets_needed: secrets_needed(&spec), spec, tool: row }
    }

    /// `(relative path, bytes)` of the tool file, and its `network`.
    fn tool_file(&self) -> (String, Vec<u8>, Option<String>) {
        let network = self.spec.get("network").and_then(Value::as_str).map(str::to_string);
        let only_source = self
            .spec
            .as_object()
            .is_some_and(|o| o.keys().all(|k| matches!(k.as_str(), "source" | "network" | "description")));
        match (self.tool.kind.as_str(), self.spec.get("source").and_then(Value::as_str)) {
            ("python", Some(src)) if only_source => (format!("tools/{}.py", self.tool.name), src.as_bytes().to_vec(), network),
            _ => {
                let mut text = serde_json::to_string_pretty(&self.spec).unwrap_or_default();
                text.push('\n');
                (format!("tools/{}.json", self.tool.name), text.into_bytes(), network)
            }
        }
    }
}

/// Environment variable `proof.sh` reads a secret's value from.
pub fn proof_secret_env(name: &str) -> String {
    let up: String = name.chars().map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_uppercase() } else { '_' }).collect();
    format!("PROOF_SECRET_{up}")
}

fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

const PROOF_TEMPLATE: &str = r#"#!/usr/bin/env bash
# proof.sh -- generated by host.example.propose (proposal @PROPOSAL@ from tenant @TENANT@).
# Signs up a fresh tenant, forks the example, sets each secret from
# PROOF_SECRET_<NAME> (skipped with `CHECK ... SKIP needs-secret` when unset),
# runs the proof call, asserts `expect`, offboards.
# Requires: bash, python3. Env: MCPHOST_URL (default https://mcphost.dev/mcp).
set -uo pipefail
cd "$(dirname "$0")"
export MCPHOST_URL="${MCPHOST_URL:-https://mcphost.dev/mcp}"
export EXAMPLE_NAME=@NAME@ PROOF_TOOL=@TOOL@ PROOF_ARGS=@ARGS@ PROOF_EXPECT=@EXPECT@ PROOF_SECRETS=@SECRETS@
python3 - <<'PY'
import json, os, sys, time, urllib.request

URL = os.environ["MCPHOST_URL"]
failures = 0

def rpc(tool, args, key=None):
    body = {"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": tool, "arguments": args, "_meta": {
        "io.modelcontextprotocol/protocolVersion": "2026-07-28",
        "io.modelcontextprotocol/clientCapabilities": {},
        "io.modelcontextprotocol/clientInfo": {"name": "example-proof", "version": "1.0.0"}}}}
    headers = {"Content-Type": "application/json", "Accept": "application/json, text/event-stream",
               "MCP-Protocol-Version": "2026-07-28", "Mcp-Method": "tools/call", "Mcp-Name": tool}
    if key:
        headers["Authorization"] = "Bearer " + key
    req = urllib.request.Request(URL, json.dumps(body).encode(), headers)
    raw = urllib.request.urlopen(req, timeout=30).read().decode()
    if raw.lstrip().startswith("data:"):
        raw = [l[5:].strip() for l in raw.splitlines() if l.startswith("data:")][-1]
    msg = json.loads(raw)
    if "error" in msg:
        return {"__error__": msg["error"]}
    return (msg.get("result") or {}).get("structuredContent")

def check(desc, ok):
    global failures
    print("CHECK %s: %s" % (desc, "PASS" if ok else "FAIL"))
    failures += 0 if ok else 1

def lookup(value, path):
    for part in path.split("."):
        if isinstance(value, list) and part.isdigit() and int(part) < len(value):
            value = value[int(part)]
        elif isinstance(value, dict) and part in value:
            value = value[part]
        else:
            return None
    return value

def holds(expect, result):
    path, op, want = expect.split(" ", 2)
    got = lookup(result, path[len("result."):])
    try:
        want = json.loads(want)
    except ValueError:
        pass
    if op == "==":
        return got == want
    if op == "!=":
        return got != want
    return want in got if isinstance(got, list) else str(want) in str(got)

names = [n for n in os.environ["PROOF_SECRETS"].split(",") if n]
values = {n: os.environ.get("PROOF_SECRET_" + "".join(c.upper() if c.isalnum() else "_" for c in n)) for n in names}
signup = rpc("signup", {"display_name": "example-proof"}) or {}
key = signup.get("key")
check("signup", bool(key))
if key:
    fork = rpc("host.example.fork", {"name": os.environ["EXAMPLE_NAME"]}, key) or {}
    check("fork", "__error__" not in fork)
    missing = [n for n in names if not values[n]]
    for n in names:
        if values[n]:
            rpc("host.secret_set", {"name": n, "value": values[n]}, key)
        else:
            print("CHECK secret %s: SKIP needs-secret" % n)
    if missing:
        print("CHECK proof call: SKIP needs-secret")
    else:
        result = None
        for _ in range(40):
            result = rpc("host.tool_call", {"name": os.environ["PROOF_TOOL"], "args": json.loads(os.environ["PROOF_ARGS"])}, key)
            if not isinstance(result, dict) or result.get("status") != "building":
                break
            time.sleep(0.5)
        check("proof call satisfies " + os.environ["PROOF_EXPECT"], holds(os.environ["PROOF_EXPECT"], result))
    rpc("host.self_offboard", {}, key)
sys.exit(1 if failures else 0)
PY
"#;

fn render_proof(p: &Prepared, req: &Request, tenant_ns: &str, proposal_id: i64) -> String {
    PROOF_TEMPLATE
        .replace("@PROPOSAL@", &proposal_id.to_string())
        .replace("@TENANT@", tenant_ns)
        .replace("@NAME@", &sh_quote(&p.name))
        .replace("@TOOL@", &sh_quote(&p.tool.name))
        .replace("@ARGS@", &sh_quote(&req.proof_args.to_string()))
        .replace("@EXPECT@", &sh_quote(&req.expect))
        .replace("@SECRETS@", &sh_quote(&p.secrets_needed.join(",")))
}

fn toml_value(v: &Value) -> toml::Value {
    match v {
        Value::Null => toml::Value::String(String::new()),
        Value::Bool(b) => toml::Value::Boolean(*b),
        Value::Number(n) => n
            .as_i64()
            .map(toml::Value::Integer)
            .unwrap_or_else(|| toml::Value::Float(n.as_f64().unwrap_or(0.0))),
        Value::String(s) => toml::Value::String(s.clone()),
        Value::Array(a) => toml::Value::Array(a.iter().map(toml_value).collect()),
        Value::Object(m) => toml::Value::Table(m.iter().map(|(k, v)| (k.clone(), toml_value(v))).collect()),
    }
}

fn render_manifest(p: &Prepared, req: &Request, tenant_ns: &str, proposal_id: i64, proposed_at: &str) -> String {
    let summary = req
        .summary
        .clone()
        .or_else(|| p.spec.get("description").and_then(Value::as_str).map(str::to_string))
        .unwrap_or_else(|| req.title.clone());
    let (file, _, network) = p.tool_file();
    let mut tool = toml::Table::new();
    tool.insert("name".into(), p.tool.name.clone().into());
    tool.insert("kind".into(), p.tool.kind.clone().into());
    tool.insert("file".into(), file.into());
    if let Some(n) = network {
        tool.insert("network".into(), n.into());
    }
    let strings = |l: &[String]| toml::Value::Array(l.iter().cloned().map(toml::Value::String).collect());
    let mut example = toml::Table::new();
    example.insert("name".into(), p.name.clone().into());
    example.insert("title".into(), req.title.clone().into());
    example.insert("summary".into(), summary.into());
    example.insert("use_case".into(), strings(&req.use_case));
    example.insert("clients".into(), strings(&req.clients));
    example.insert("since".into(), crate::build_info::BUILD.version.to_string().into());
    example.insert("secrets_needed".into(), strings(&p.secrets_needed));
    let mut first_call = toml::Table::new();
    first_call.insert("tool".into(), p.tool.name.clone().into());
    first_call.insert("expect".into(), req.expect.clone().into());
    first_call.insert("args".into(), toml_value(&req.proof_args));
    let mut provenance = toml::Table::new();
    provenance.insert("tenant".into(), tenant_ns.to_string().into());
    provenance.insert("tool_version".into(), p.tool.current_version.into());
    provenance.insert("proposal_id".into(), proposal_id.into());
    provenance.insert("proposed_at".into(), proposed_at.to_string().into());
    provenance.insert("license".into(), LICENSE.into());
    let mut root = toml::Table::new();
    root.insert("example".into(), example.into());
    root.insert("tools".into(), toml::Value::Array(vec![tool.into()]));
    root.insert("first_call".into(), first_call.into());
    root.insert("provenance".into(), provenance.into());
    toml::to_string(&root).unwrap_or_default()
}

/// The rendered directory: relative path -> bytes.
pub type Files = BTreeMap<String, Vec<u8>>;

/// Who proposed, as which proposal, when: the `[provenance]` inputs.
#[derive(Clone, Copy)]
pub struct Stamp<'a> {
    pub tenant_ns: &'a str,
    pub proposal_id: i64,
    pub proposed_at: &'a str,
}

/// Render and validate the directory. A validation failure comes back to
/// the proposer verbatim (R3).
pub fn render_files(p: &Prepared, req: &Request, stamp: &Stamp<'_>, kinds: &[&str]) -> Result<Files, AppError> {
    let Stamp { tenant_ns, proposal_id, proposed_at } = *stamp;
    let mut files = Files::new();
    let (path, bytes, _) = p.tool_file();
    files.insert(path, bytes);
    files.insert(crate::examples::MANIFEST_FILE.to_string(), render_manifest(p, req, tenant_ns, proposal_id, proposed_at).into_bytes());
    files.insert("proof.sh".to_string(), render_proof(p, req, tenant_ns, proposal_id).into_bytes());
    if let Err(errs) = crate::examples::load_with(&p.name, kinds, &|rel| files.get(rel).cloned()) {
        let listed: Vec<String> = errs.iter().map(ToString::to_string).collect();
        return Err(AppError::Structured {
            code: INVALID_EXAMPLE,
            message: format!("the rendered example fails `mcphost examples check`: {}", listed.join("; ")),
            data: json!({"errors": listed}),
        });
    }
    Ok(files)
}

/// The in-memory tar of the directory (`example.toml`, `tools/`, `proof.sh`).
pub fn pack_tar(files: &Files) -> Result<Vec<u8>, AppError> {
    let mut builder = tar::Builder::new(Vec::new());
    for (path, bytes) in files {
        let mut header = tar::Header::new_gnu();
        header.set_size(bytes.len() as u64);
        header.set_mode(if path == "proof.sh" { 0o755 } else { 0o644 });
        header.set_mtime(0);
        header.set_cksum();
        builder
            .append_data(&mut header, path, bytes.as_slice())
            .map_err(|e| AppError::Internal(format!("example tar: {e}")))?;
    }
    let tar = builder.into_inner().map_err(|e| AppError::Internal(format!("example tar: {e}")))?;
    if tar.len() > TAR_MAX_BYTES {
        return Err(AppError::Structured {
            code: TOO_LARGE,
            message: format!("the rendered example is {} bytes; the limit is {TAR_MAX_BYTES}", tar.len()),
            data: json!({"bytes": tar.len(), "max_bytes": TAR_MAX_BYTES}),
        });
    }
    Ok(tar)
}

/// One file of a stored tar: path, bytes, mode.
pub type TarEntry = (String, Vec<u8>, u32);

/// Unpack a stored tar back into its files.
pub fn unpack_tar(tar_bytes: &[u8]) -> Result<Vec<TarEntry>, AppError> {
    use std::io::Read;
    let mut archive = tar::Archive::new(tar_bytes);
    let mut out = Vec::new();
    let entries = archive.entries().map_err(|e| AppError::Internal(format!("example tar: {e}")))?;
    for entry in entries {
        let mut entry = entry.map_err(|e| AppError::Internal(format!("example tar: {e}")))?;
        let path = entry.path().map_err(|e| AppError::Internal(format!("example tar: {e}")))?.to_string_lossy().into_owned();
        let mode = entry.header().mode().unwrap_or(0o644);
        let mut bytes = Vec::new();
        entry.read_to_end(&mut bytes).map_err(|e| AppError::Internal(format!("example tar: {e}")))?;
        out.push((path, bytes, mode));
    }
    Ok(out)
}

// ------------------------------------------------------------- portability

/// Substrings that tie a tool to its author's own tenant data: the state
/// store, tables and groups, in dotted, flattened and python-bridge forms.
const TENANT_BOUND: &[&str] = &[
    "host.state.", "host.state_", "host_state_", "mcphost.state",
    "host.table.", "host.table_", "host_table_", "mcphost.table",
    "host.group.", "host.group_", "host_group_", "mcphost.group",
];

/// The `[A-Za-z0-9_.]` token that starts at `at` in `text`.
fn token_at(text: &str, at: usize) -> String {
    text[at..]
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '.')
        .collect::<String>()
        .trim_end_matches('.')
        .to_string()
}

fn other_tenants_tool(v: &Value, own_ns: &str) -> Option<String> {
    match v {
        Value::Object(m) => {
            if let Some(Value::String(t)) = m.get("tool")
                && let Some((ns, _)) = t.split_once('.')
                && ns != "host"
                && ns != "billing"
                && ns != own_ns
            {
                return Some(t.clone());
            }
            m.values().find_map(|x| other_tenants_tool(x, own_ns))
        }
        Value::Array(a) => a.iter().find_map(|x| other_tenants_tool(x, own_ns)),
        _ => None,
    }
}

/// R1: the first reference that makes the tool depend on its author's
/// tenant (state tables, groups, another tenant's shared tool), if any.
pub fn find_non_portable(tool_text: &str, spec: &Value, own_ns: &str) -> Option<String> {
    let first = TENANT_BOUND
        .iter()
        .filter_map(|needle| tool_text.find(needle))
        .min()
        .map(|at| token_at(tool_text, at));
    first.or_else(|| other_tenants_tool(spec, own_ns))
}

fn not_portable(reference: &str) -> AppError {
    AppError::Structured {
        code: NOT_PORTABLE,
        message: format!(
            "the tool references `{reference}`, which lives in your tenant; an example must run in a fresh one"
        ),
        data: json!({"reference": reference}),
    }
}

// ------------------------------------------------------------ secret scrub

/// Secrets shorter than this are not scanned for: a 1-3 byte "secret"
/// matches ordinary source text and refusing on it would block every tool.
pub const MIN_SCANNED_SECRET_BYTES: usize = 4;

/// Does `hay` contain `needle`? Every window is compared and the result
/// folded without an early exit, so timing does not reveal where, or how
/// much of, a stored secret matched.
fn contains_constant_time(hay: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() || needle.len() > hay.len() {
        return false;
    }
    let mut found = 0u8;
    for window in hay.windows(needle.len()) {
        let diff = window.iter().zip(needle).fold(0u8, |acc, (a, b)| acc | (a ^ b));
        found |= u8::from(diff == 0);
    }
    found == 1
}

/// R1: the first line of any file that is about to leave the tenant holding
/// the literal value of a stored secret. Returns the file, the 1-based line
/// and the secret's NAME; the value is never returned.
pub fn find_secret_leak(files: &[(&str, &str)], secrets: &[(String, String)]) -> Option<(String, usize, String)> {
    for (file, text) in files {
        for (i, line) in text.lines().enumerate() {
            for (name, value) in secrets.iter().filter(|(_, v)| v.len() >= MIN_SCANNED_SECRET_BYTES) {
                if contains_constant_time(line.as_bytes(), value.as_bytes()) {
                    return Some((file.to_string(), i + 1, name.clone()));
                }
            }
        }
    }
    None
}

fn secret_leak(file: &str, line: usize, name: &str) -> AppError {
    AppError::Structured {
        code: SECRET_LEAK,
        message: format!(
            "line {line} of {file} holds the literal value of your stored secret `{name}`; \
             replace it with {{{{ secret.{name} }}}} and republish before proposing"
        ),
        data: json!({"line": line, "file": file, "secret": name}),
    }
}

// ----------------------------------------------------------------- expect

fn lookup<'a>(value: &'a Value, path: &str) -> Option<&'a Value> {
    let mut cur = value;
    for part in path.split('.') {
        cur = match cur {
            Value::Array(a) => a.get(part.parse::<usize>().ok()?)?,
            Value::Object(m) => m.get(part)?,
            _ => return None,
        };
    }
    Some(cur)
}

/// Does `result` satisfy `expect` (`result.<path> <op> <value>`, the
/// manifest grammar)? `Err` carries what was actually found.
pub fn eval_expect(expect: &str, result: &Value) -> Result<(), String> {
    let mut parts = expect.splitn(3, ' ');
    let (Some(path), Some(op), Some(raw)) = (parts.next(), parts.next(), parts.next()) else {
        return Err(format!("`{expect}` is not `result.<path> <op> <value>`"));
    };
    let path = path.strip_prefix("result.").unwrap_or(path);
    let want = serde_json::from_str::<Value>(raw).unwrap_or_else(|_| Value::String(raw.to_string()));
    let got = lookup(result, path);
    let equal = |g: &Value| match (g, &want) {
        (Value::Number(a), Value::Number(b)) => a.as_f64() == b.as_f64(),
        (Value::String(s), w) if !w.is_string() => s == raw,
        (g, w) => g == w,
    };
    let holds = match (op, got) {
        ("==", Some(g)) => equal(g),
        ("!=", Some(g)) => !equal(g),
        ("!=", None) => true,
        ("contains", Some(Value::Array(a))) => a.iter().any(equal),
        ("contains", Some(Value::String(s))) => s.contains(want.as_str().unwrap_or(raw)),
        _ => false,
    };
    if holds { Ok(()) } else { Err(format!("`{expect}` does not hold; result.{path} is {}", got.map_or("absent".to_string(), Value::to_string))) }
}

fn proof_failed(expect: &str, why: &str, actual: &str) -> AppError {
    AppError::Structured {
        code: PROOF_FAILED,
        message: format!("proof_call does not hold now: {why}; actual result: {actual}"),
        data: json!({"expect": expect, "actual": actual}),
    }
}

/// A short excerpt of `v` with every stored secret value masked.
pub fn excerpt(v: &Value, secrets: &[(String, String)]) -> String {
    let mut text = v.to_string();
    for (_, value) in secrets.iter().filter(|(_, v)| !v.is_empty()) {
        text = text.replace(value.as_str(), "***");
    }
    if text.chars().count() > EXCERPT_MAX_CHARS {
        text = text.chars().take(EXCERPT_MAX_CHARS).collect::<String>() + "...";
    }
    text
}

/// The tenant's stored secrets, decrypted server-side only.
pub async fn tenant_secrets(state: &AppState, tenant_id: i64) -> Result<Vec<(String, String)>, AppError> {
    let mut out = Vec::new();
    for name in state.db.list_secret_names(tenant_id).await? {
        if let Some((enc, nonce)) = state.db.get_secret(tenant_id, name.clone()).await? {
            out.push((name, state.secrets.decrypt(&enc, &nonce)?));
        }
    }
    Ok(out)
}

/// The proposal as the proposer and the admin list see it.
pub fn row_json(row: &ExampleProposalRow) -> Value {
    json!({
        "id": row.id,
        "tenant": row.tenant,
        "tool": row.tool,
        "name": row.name,
        "version": row.version,
        "status": row.status,
        "created_at": row.created_at,
        "decided_at": row.decided_at,
        "decided_by": row.decided_by,
        "reason": row.reason,
        "pr_url": row.pr_url,
    })
}

pub fn file_listing(files: &Files) -> Vec<String> {
    files.keys().cloned().collect()
}

/// `path -> text` of the rendered files, for the proposer to read and diff.
pub fn file_contents(files: &Files) -> Value {
    Value::Object(files.iter().map(|(k, v)| (k.clone(), json!(String::from_utf8_lossy(v)))).collect())
}

// ---------------------------------------------------------------- pipeline

/// Everything refused before the proof call runs: `example_exists`,
/// `too_many_proposals`, the tool's existence.
pub async fn precheck(state: &AppState, tenant: &crate::db::Tenant, req: &Request) -> Result<Prepared, AppError> {
    if exists_in_index(&req.tool) {
        return Err(example_exists(&req.tool));
    }
    let open = state.db.count_open_example_proposals(tenant.id).await?;
    if open >= MAX_OPEN_PROPOSALS {
        return Err(too_many_proposals(open));
    }
    let row = state
        .db
        .get_tool(tenant.id, req.tool.clone())
        .await?
        .ok_or_else(|| AppError::ToolNotFound(req.tool.clone()))?;
    let prepared = Prepared::build(row);
    let (path, bytes, _) = prepared.tool_file();
    let tool_text = String::from_utf8_lossy(&bytes).into_owned();
    if let Some(reference) = find_non_portable(&tool_text, &prepared.spec, &tenant.namespace) {
        return Err(not_portable(&reference));
    }
    let secrets = tenant_secrets(state, tenant.id).await?;
    let proof_text = json!({"args": req.proof_args, "expect": req.expect}).to_string();
    let candidates = [
        (path.as_str(), tool_text.as_str()),
        ("title", req.title.as_str()),
        ("summary", req.summary.as_deref().unwrap_or("")),
        ("proof_call", proof_text.as_str()),
    ];
    if let Some((file, line, name)) = find_secret_leak(&candidates, &secrets) {
        return Err(secret_leak(&file, line, &name));
    }
    Ok(prepared)
}

/// After the proof call: check it, render, validate, store, answer.
pub async fn finish(
    state: &AppState,
    tenant: &crate::db::Tenant,
    req: &Request,
    prepared: Prepared,
    proof: Result<Value, AppError>,
) -> Result<Value, AppError> {
    let secrets = tenant_secrets(state, tenant.id).await?;
    let result = match proof {
        Ok(v) => v,
        Err(e) => {
            return Err(proof_failed(&req.expect, &format!("the proof call failed: {}", e.code()), &excerpt(&json!(e.to_string()), &secrets)));
        }
    };
    if let Err(why) = eval_expect(&req.expect, &result) {
        return Err(proof_failed(&req.expect, &why, &excerpt(&result, &secrets)));
    }
    let kinds = state.kinds.names();
    let proposed_at = crate::state::rfc3339_now();
    // Validate with a placeholder id first so a bad rendering never costs a row.
    render_files(&prepared, req, &Stamp { tenant_ns: &tenant.namespace, proposal_id: 0, proposed_at: &proposed_at }, &kinds)?;
    let id = state
        .db
        .insert_example_proposal(
            tenant.id,
            prepared.tool.name.clone(),
            prepared.name.clone(),
            prepared.tool.current_version,
            Vec::new(),
            crate::state::now_unix(),
            MAX_OPEN_PROPOSALS,
        )
        .await?
        .map_err(too_many_proposals)?;
    let stored = async {
        let files = render_files(&prepared, req, &Stamp { tenant_ns: &tenant.namespace, proposal_id: id, proposed_at: &proposed_at }, &kinds)?;
        let tar = pack_tar(&files)?;
        state.db.set_example_proposal_tar(id, tar).await?;
        Ok::<Files, AppError>(files)
    }
    .await;
    let files = match stored {
        Ok(f) => f,
        Err(e) => {
            state.db.delete_example_proposal(id).await?;
            return Err(e);
        }
    };
    let example_toml = String::from_utf8_lossy(files.get(crate::examples::MANIFEST_FILE).map_or(&[][..], Vec::as_slice)).into_owned();
    Ok(json!({
        "proposal_id": id,
        "status": "pending",
        "name": prepared.name,
        "secrets_needed": prepared.secrets_needed,
        "rendered": {"files": file_listing(&files), "example_toml": example_toml, "contents": file_contents(&files)},
    }))
}

/// `host.example.proposal_status {id}`: the proposer's own proposals only.
pub async fn proposal_status(state: &AppState, tenant: &crate::db::Tenant, args: &Value) -> Result<Value, AppError> {
    let id = args
        .get("id")
        .and_then(Value::as_i64)
        .ok_or_else(|| AppError::InvalidArgs("missing required argument 'id'".into()))?;
    match state.db.get_example_proposal(id).await? {
        Some(row) if row.tenant_id == tenant.id => Ok(row_json(&row)),
        _ => Err(AppError::ToolNotFound(format!("example proposal {id}"))),
    }
}

crate::err_code! {
    pub EXAMPLE_EXISTS,
    code: "example_exists", family: Share, class: AgentFault, retryable: false,
    remedy: "Rename the tool and propose again.",
}

crate::err_code! {
    pub INVALID_EXAMPLE,
    code: "invalid_example", family: Share, class: AgentFault, retryable: false,
    remedy: "Fix the errors listed in data.errors and propose again.",
}

crate::err_code! {
    pub NOT_PORTABLE,
    code: "not_portable", family: Share, class: AgentFault, retryable: false,
    remedy: "Remove the reference to your tenant named in data.reference and propose again.",
}

crate::err_code! {
    pub PROOF_FAILED,
    code: "proof_failed", family: Share, class: AgentFault, retryable: false,
    remedy: "Fix the tool so proof_call holds; data.actual is the result it returned.",
}

crate::err_code! {
    pub SECRET_LEAK,
    code: "secret_leak", family: Share, class: AgentFault, retryable: false,
    remedy: "Replace the literal value with the secret placeholder named in the message and republish.",
}

crate::err_code! {
    pub TOO_LARGE,
    code: "too_large", family: Plan, class: PlanLimit, retryable: false,
    remedy: "Shrink the payload to the limit in data.max_bytes.",
}

crate::err_code! {
    pub TOO_MANY_PROPOSALS,
    code: "too_many_proposals", family: Plan, class: PlanLimit, retryable: false,
    remedy: "Wait for an open proposal to be decided; data.limit is the maximum.",
}

