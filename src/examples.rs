//! PRD-mcphost-example-manifest-contract: every `examples/<name>/` directory
//! is described by one `example.toml`; this module is the only place that
//! reads them. The use-case and client vocabularies live here once
//! ([`EXAMPLE_USE_CASES`], [`CLIENT_TAGS`]); everything else derives from them.

use std::fmt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// R2: the closed use-case vocabulary a manifest's `use_case` draws from.
pub const EXAMPLE_USE_CASES: &[&str] = &[
    "uptime", "database", "docs-qa", "webhook", "chart", "memory", "share", "first-run",
];

/// R2: the closed client vocabulary a manifest's `clients` draws from.
pub const CLIENT_TAGS: &[&str] = &[
    "claude_code",
    "claude_ai",
    "chatgpt",
    "cursor",
    "copilot_vscode",
    "codex",
    "gemini_cli",
    "any",
];

/// File name of the per-example manifest.
pub const MANIFEST_FILE: &str = "example.toml";

const TITLE_MAX: usize = 60;
const SUMMARY_MAX: usize = 200;
const EXPECT_OPS: &[&str] = &["==", "!=", "contains"];

/// One validation failure: which directory, which field, why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManifestError {
    pub dir: String,
    pub field: String,
    pub reason: String,
}

impl fmt::Display for ManifestError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "examples/{}: field `{}`: {}", self.dir, self.field, self.reason)
    }
}

fn err(dir: &str, field: &str, reason: impl Into<String>) -> ManifestError {
    ManifestError { dir: dir.to_string(), field: field.to_string(), reason: reason.into() }
}

// ---------------------------------------------------------------- manifest

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    example: Meta,
    #[serde(default)]
    tools: Vec<ToolEntry>,
    first_call: FirstCall,
    /// PRD-mcphost-example-from-published-tool R2: present only on an
    /// example a tenant proposed; hand-written examples omit it.
    provenance: Option<Provenance>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Provenance {
    tenant: String,
    tool_version: i64,
    proposal_id: i64,
    proposed_at: String,
    license: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Meta {
    name: String,
    title: String,
    summary: String,
    use_case: Vec<String>,
    clients: Vec<String>,
    since: String,
    #[serde(default = "default_proof")]
    proof: String,
    #[serde(default)]
    secrets_needed: Vec<String>,
}

fn default_proof() -> String {
    "proof.sh".to_string()
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ToolEntry {
    name: String,
    kind: String,
    file: String,
    network: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct FirstCall {
    tool: String,
    #[serde(default)]
    args: toml::Table,
    expect: String,
}

// ------------------------------------------------------------------- index

/// One validated example, as written to `index.json`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct IndexEntry {
    pub name: String,
    pub title: String,
    pub summary: String,
    pub use_case: Vec<String>,
    pub clients: Vec<String>,
    pub since: String,
    pub proof: String,
    pub secrets_needed: Vec<String>,
    pub manifest_sha256: String,
    pub tools: Vec<IndexTool>,
    pub first_call: IndexFirstCall,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provenance: Option<IndexProvenance>,
}

/// `[provenance]` as written to `index.json`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct IndexProvenance {
    pub tenant: String,
    pub tool_version: i64,
    pub proposal_id: i64,
    pub proposed_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub license: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct IndexTool {
    pub name: String,
    pub kind: String,
    pub file: String,
    pub sha256: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub network: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct IndexFirstCall {
    pub tool: String,
    pub args: serde_json::Value,
    pub expect: String,
}

/// Every kind name the production registry registers (`serve` registers
/// echo, http, python, chain, wasm), so a kind removed from the registry
/// fails `examples check`.
pub fn registered_kind_names() -> Vec<&'static str> {
    let mut kinds = crate::kinds::KindRegistry::with_builtin();
    if let Ok((http, python)) =
        crate::kinds::network_kinds("https://mcphost.dev", &std::env::temp_dir().join("mcphost-examples-kinds"))
    {
        kinds.register(std::sync::Arc::new(http));
        kinds.register(std::sync::Arc::new(python));
    }
    kinds.register(std::sync::Arc::new(crate::kinds::chain::ChainKind));
    kinds.register(std::sync::Arc::new(crate::kinds::wasm::WasmKind::new()));
    kinds.names()
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

fn is_relative_inside(p: &str) -> bool {
    let path = Path::new(p);
    !p.is_empty() && path.is_relative() && !path.components().any(|c| matches!(c, std::path::Component::ParentDir))
}

/// `result.<path> <op> <value>`, ops `==`, `!=`, `contains`.
fn check_expect(expect: &str) -> Result<(), String> {
    let mut parts = expect.splitn(3, ' ');
    let (Some(path), Some(op), Some(value)) = (parts.next(), parts.next(), parts.next()) else {
        return Err(format!("`{expect}` is not `result.<path> <op> <value>`"));
    };
    let Some(rest) = path.strip_prefix("result.") else {
        return Err(format!("`{path}` must start with `result.`"));
    };
    if rest.is_empty() || !rest.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.') {
        return Err(format!("`{path}` is not a dotted result path"));
    }
    if !EXPECT_OPS.contains(&op) {
        return Err(format!("operator `{op}` is not one of {}", EXPECT_OPS.join(", ")));
    }
    if value.trim().is_empty() {
        return Err("value is empty".to_string());
    }
    Ok(())
}

fn check_vocab(dir: &str, field: &str, values: &[String], allowed: &[&str], errs: &mut Vec<ManifestError>) {
    if values.is_empty() {
        errs.push(err(dir, field, "must list at least one value"));
    }
    for v in values {
        if !allowed.contains(&v.as_str()) {
            errs.push(err(dir, field, format!("`{v}` is not one of {}", allowed.join(", "))));
        }
    }
}

/// Validate one example directory against the schema rules and the
/// filesystem. All failures for the directory are returned.
pub fn load_one(dir: &Path, kinds: &[&str]) -> Result<IndexEntry, Vec<ManifestError>> {
    let dname = dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    load_with(&dname, kinds, &|rel| {
        if is_relative_inside(rel) { std::fs::read(dir.join(rel)).ok() } else { None }
    })
}

/// [`load_one`] over any file source: `read(relative_path)` yields the file's
/// bytes, `None` when it does not exist. `propose` validates an in-memory
/// rendering with the very code `mcphost examples check` runs.
pub fn load_with(
    dname: &str,
    kinds: &[&str],
    read: &dyn Fn(&str) -> Option<Vec<u8>>,
) -> Result<IndexEntry, Vec<ManifestError>> {
    let Some(raw) = read(MANIFEST_FILE) else {
        return Err(vec![err(dname, MANIFEST_FILE, "directory has no example.toml")]);
    };
    let text = match String::from_utf8(raw.clone()) {
        Ok(t) => t,
        Err(_) => return Err(vec![err(dname, MANIFEST_FILE, "not valid UTF-8")]),
    };
    let m: Manifest = match toml::from_str(&text) {
        Ok(m) => m,
        Err(e) => return Err(vec![err(dname, MANIFEST_FILE, e.message().to_string())]),
    };
    let mut errs = Vec::new();
    let meta = &m.example;
    if meta.name != dname {
        errs.push(err(dname, "example.name", format!("`{}` must equal the directory name", meta.name)));
    }
    for (field, value, max) in [("example.title", &meta.title, TITLE_MAX), ("example.summary", &meta.summary, SUMMARY_MAX)] {
        let n = value.chars().count();
        if n == 0 || n > max {
            errs.push(err(dname, field, format!("length {n} must be 1..={max}")));
        }
    }
    check_vocab(dname, "example.use_case", &meta.use_case, EXAMPLE_USE_CASES, &mut errs);
    check_vocab(dname, "example.clients", &meta.clients, CLIENT_TAGS, &mut errs);
    let since_ok = {
        let p: Vec<&str> = meta.since.split('.').collect();
        p.len() == 3 && p.iter().all(|s| !s.is_empty() && s.chars().all(|c| c.is_ascii_digit()))
    };
    if !since_ok {
        errs.push(err(dname, "example.since", format!("`{}` is not a MAJOR.MINOR.PATCH version", meta.since)));
    }
    if !is_relative_inside(&meta.proof) || read(&meta.proof).is_none() {
        errs.push(err(dname, "example.proof", format!("file `{}` does not exist in the directory", meta.proof)));
    }
    let mut tools = Vec::new();
    for (i, t) in m.tools.iter().enumerate() {
        let field = |f: &str| format!("tools[{i}].{f}");
        if t.name.is_empty() {
            errs.push(err(dname, &field("name"), "is empty"));
        }
        if !kinds.contains(&t.kind.as_str()) {
            errs.push(err(dname, &field("kind"), format!("`{}` is not a registered kind ({})", t.kind, kinds.join(", "))));
        }
        let bytes = if is_relative_inside(&t.file) { read(&t.file) } else { None };
        match bytes {
            Some(b) => tools.push(IndexTool {
                name: t.name.clone(),
                kind: t.kind.clone(),
                file: t.file.clone(),
                sha256: sha256_hex(&b),
                network: t.network.clone(),
            }),
            None => errs.push(err(dname, &field("file"), format!("file `{}` does not exist in the directory", t.file))),
        }
    }
    if m.first_call.tool.is_empty() {
        errs.push(err(dname, "first_call.tool", "is empty"));
    }
    if let Err(why) = check_expect(&m.first_call.expect) {
        errs.push(err(dname, "first_call.expect", why));
    }
    let args = serde_json::to_value(&m.first_call.args).unwrap_or(serde_json::Value::Null);
    if !errs.is_empty() {
        return Err(errs);
    }
    Ok(IndexEntry {
        name: meta.name.clone(),
        title: meta.title.clone(),
        summary: meta.summary.clone(),
        use_case: meta.use_case.clone(),
        clients: meta.clients.clone(),
        since: meta.since.clone(),
        proof: meta.proof.clone(),
        secrets_needed: meta.secrets_needed.clone(),
        manifest_sha256: sha256_hex(&raw),
        tools,
        first_call: IndexFirstCall { tool: m.first_call.tool.clone(), args, expect: m.first_call.expect.clone() },
        provenance: m.provenance.map(|p| IndexProvenance {
            tenant: p.tenant,
            tool_version: p.tool_version,
            proposal_id: p.proposal_id,
            proposed_at: p.proposed_at,
            license: p.license,
        }),
    })
}

/// Every immediate subdirectory of `examples_dir`, sorted by name.
pub fn example_dirs(examples_dir: &Path) -> std::io::Result<Vec<PathBuf>> {
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(examples_dir)?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    dirs.sort();
    Ok(dirs)
}

/// Validate every `examples_dir/*/example.toml`; a directory without a
/// manifest is a failure. Entries come back sorted by name.
pub fn collect(examples_dir: &Path, kinds: &[&str]) -> Result<Vec<IndexEntry>, Vec<ManifestError>> {
    let dirs = example_dirs(examples_dir)
        .map_err(|e| vec![err("", "examples", format!("cannot read {}: {e}", examples_dir.display()))])?;
    let mut entries = Vec::new();
    let mut errs = Vec::new();
    for d in dirs {
        match load_one(&d, kinds) {
            Ok(e) => entries.push(e),
            Err(mut es) => errs.append(&mut es),
        }
    }
    if errs.is_empty() { Ok(entries) } else { Err(errs) }
}

/// File name of the generated index, next to the example directories.
pub const INDEX_FILE: &str = "index.json";

#[derive(Serialize)]
struct IndexFile<'a> {
    examples: &'a [IndexEntry],
}

/// The exact bytes of `index.json` for these entries (sorted by name,
/// two-space indent, trailing newline).
pub fn render_index(entries: &[IndexEntry]) -> String {
    let mut s = serde_json::to_string_pretty(&IndexFile { examples: entries }).expect("index serializes");
    s.push('\n');
    s
}

/// `mcphost examples index`: validate everything, then write
/// `examples_dir/index.json`. Nothing is written when any manifest fails.
pub fn write_index(examples_dir: &Path, kinds: &[&str]) -> Result<usize, Vec<ManifestError>> {
    let entries = collect(examples_dir, kinds)?;
    std::fs::write(examples_dir.join(INDEX_FILE), render_index(&entries))
        .map_err(|e| vec![err("", INDEX_FILE, format!("cannot write: {e}"))])?;
    Ok(entries.len())
}

// ---------------------------------------------------------------- embedded

/// One example as embedded into the binary at build time from
/// `examples/index.json` (R4); tool sources are the files themselves.
#[derive(Debug, Clone, Copy)]
pub struct Example {
    pub name: &'static str,
    pub title: &'static str,
    pub summary: &'static str,
    pub use_case: &'static [&'static str],
    pub clients: &'static [&'static str],
    pub since: &'static str,
    pub proof: &'static str,
    pub secrets_needed: &'static [&'static str],
    pub tools: &'static [ExampleTool],
    pub first_call: ExampleFirstCall,
}

#[derive(Debug, Clone, Copy)]
pub struct ExampleTool {
    pub name: &'static str,
    pub kind: &'static str,
    pub file: &'static str,
    pub source: &'static str,
    pub network: Option<&'static str>,
}

#[derive(Debug, Clone, Copy)]
pub struct ExampleFirstCall {
    pub tool: &'static str,
    /// The call's arguments as a JSON object string.
    pub args_json: &'static str,
    pub expect: &'static str,
}

include!(concat!(env!("OUT_DIR"), "/examples_embedded.rs"));

// ------------------------------------------------------------------ schema

/// File name of the generated manifest schema, next to the index.
pub const SCHEMA_FILE: &str = "example.schema.json";

/// The JSON schema of `example.toml` with the given vocabularies inlined.
/// [`render_schema`] feeds it the real slices; a drift test feeds it a
/// changed one to prove the committed file would no longer match.
pub fn render_schema_with(use_cases: &[&str], clients: &[&str]) -> String {
    let schema = serde_json::json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "title": "mcphost example manifest (examples/<name>/example.toml)",
        "type": "object",
        "additionalProperties": false,
        "required": ["example", "first_call"],
        "properties": {
            "example": {
                "type": "object",
                "additionalProperties": false,
                "required": ["name", "title", "summary", "use_case", "clients", "since"],
                "properties": {
                    "name": { "type": "string", "minLength": 1, "description": "must equal the directory name" },
                    "title": { "type": "string", "minLength": 1, "maxLength": TITLE_MAX },
                    "summary": { "type": "string", "minLength": 1, "maxLength": SUMMARY_MAX },
                    "use_case": {
                        "type": "array",
                        "minItems": 1,
                        "items": { "type": "string", "enum": use_cases }
                    },
                    "clients": {
                        "type": "array",
                        "minItems": 1,
                        "items": { "type": "string", "enum": clients }
                    },
                    "since": { "type": "string", "pattern": "^[0-9]+\\.[0-9]+\\.[0-9]+$" },
                    "proof": { "type": "string", "default": "proof.sh", "description": "relative path of the proof script" },
                    "secrets_needed": { "type": "array", "items": { "type": "string" }, "default": [] }
                }
            },
            "tools": {
                "type": "array",
                "items": {
                    "type": "object",
                    "additionalProperties": false,
                    "required": ["name", "kind", "file"],
                    "properties": {
                        "name": { "type": "string", "minLength": 1 },
                        "kind": { "type": "string", "description": "a name from KindRegistry::names(), checked by `mcphost examples check`" },
                        "file": { "type": "string", "description": "path relative to the example directory" },
                        "network": { "type": "string" }
                    }
                }
            },
            "first_call": {
                "type": "object",
                "additionalProperties": false,
                "required": ["tool", "expect"],
                "properties": {
                    "tool": { "type": "string", "minLength": 1 },
                    "args": { "type": "object" },
                    "expect": { "type": "string", "description": "result.<path> <op> <value>; op is one of ==, !=, contains" }
                }
            },
            "provenance": {
                "type": "object",
                "additionalProperties": false,
                "required": ["tenant", "tool_version", "proposal_id", "proposed_at"],
                "description": "present only on an example a tenant proposed (host.example.propose)",
                "properties": {
                    "tenant": { "type": "string", "minLength": 1 },
                    "tool_version": { "type": "integer" },
                    "proposal_id": { "type": "integer" },
                    "proposed_at": { "type": "string" },
                    "license": { "type": "string" }
                }
            }
        }
    });
    let mut s = serde_json::to_string_pretty(&schema).expect("schema serializes");
    s.push('\n');
    s
}

/// The bytes of `examples/example.schema.json` (`mcphost examples schema`).
pub fn render_schema() -> String {
    render_schema_with(EXAMPLE_USE_CASES, CLIENT_TAGS)
}
