//! PRD-mcphost-tool-directory-format: a tool as a directory. `tool.toml`
//! carries `[tool] name, kind, description?` and `[spec]` (every non-file
//! spec field); `tool.py` carries a python tool's `source`; `tool.wasm`
//! carries a wasm tool's raw `component` bytes; `tests/<name>.json` carries
//! declared `{args, expect?}` invocations (never part of the publish
//! payload). [`pack`] emits the exact `host.tool_publish` payload,
//! [`unpack`] writes one back, [`lint`] runs the server's own validators.
//!
//! The field table in `docs/tool-directory.md` ([`render_docs`]) is rendered
//! from `tool_publish_props` and each kind's `known_spec_fields`, so it
//! cannot drift from the registry.

use std::path::{Path, PathBuf};

use base64::Engine;
use serde_json::{Map, Value, json};

use crate::kinds::{KindRegistry, UNIVERSAL_SPEC_FIELDS, check_unknown_spec_field};

pub const TOOL_TOML: &str = "tool.toml";
pub const TOOL_PY: &str = "tool.py";
pub const TOOL_WASM: &str = "tool.wasm";
pub const TESTS_DIR: &str = "tests";
pub const DOC_PATH: &str = "docs/tool-directory.md";

/// Payload keys besides `name`/`kind`/`spec` that `host.tool_publish`
/// accepts and a directory can carry (in `[tool]`).
const EXTRA_TOOL_KEYS: &[&str] = &["scopes", "visibility", "tags"];

/// A pack/unpack/lint failure: a stable `code`, a human `message`, and an
/// optional machine-readable `error_code` (the server's own code for a
/// structured kind error such as `unknown_spec_field`) plus `data`.
#[derive(Debug, Clone)]
pub struct TooldirError {
    pub code: &'static str,
    pub message: String,
    pub error_code: Option<&'static str>,
    pub data: Value,
}

impl TooldirError {
    fn invalid(message: impl Into<String>) -> Self {
        Self { code: crate::errors::INVALID_SPEC, message: message.into(), error_code: None, data: Value::Null }
    }

    /// The one-line JSON the CLI prints on stdout.
    pub fn to_json(&self) -> Value {
        let mut out = json!({"ok": false, "code": self.code, "message": self.message});
        if let Some(obj) = out.as_object_mut() {
            if let Some(ec) = self.error_code {
                obj.insert("error_code".into(), json!(ec));
            }
            if !self.data.is_null() {
                obj.insert("data".into(), self.data.clone());
            }
        }
        out
    }
}

/// The spec field a kind keeps in its own file, and how the file maps to it.
enum FileField {
    Text(&'static str, &'static str),
    Base64(&'static str, &'static str),
}

fn file_field(kind: &str) -> Option<FileField> {
    match kind {
        "python" => Some(FileField::Text("source", TOOL_PY)),
        "wasm" => Some(FileField::Base64("component", TOOL_WASM)),
        _ => None,
    }
}

fn toml_to_json(value: toml::Value) -> Result<Value, TooldirError> {
    serde_json::to_value(value).map_err(|e| TooldirError::invalid(format!("tool.toml: {e}")))
}

fn json_to_toml(value: &Value) -> Result<toml::Value, TooldirError> {
    toml::Value::try_from(value)
        .map_err(|e| TooldirError::invalid(format!("{TOOL_TOML}: cannot be written as TOML ({e}); TOML has no null")))
}

/// Reads `dir` into the `host.tool_publish` payload `{name, kind, spec}`
/// (plus `scopes`/`visibility`/`tags` when `[tool]` names them). Writes
/// nothing.
pub fn pack(dir: &Path) -> Result<Value, TooldirError> {
    let toml_path = dir.join(TOOL_TOML);
    let text = std::fs::read_to_string(&toml_path)
        .map_err(|e| TooldirError::invalid(format!("{}: cannot read {TOOL_TOML}: {e}", dir.display())))?;
    let doc: toml::Table = text
        .parse()
        .map_err(|e| TooldirError::invalid(format!("{TOOL_TOML}: not valid TOML: {e}")))?;
    let tool = match doc.get("tool") {
        Some(toml::Value::Table(t)) => t.clone(),
        _ => return Err(TooldirError::invalid(format!("{TOOL_TOML}: missing the [tool] table"))),
    };
    let str_key = |key: &str| -> Result<String, TooldirError> {
        match tool.get(key) {
            Some(toml::Value::String(s)) if !s.is_empty() => Ok(s.clone()),
            _ => Err(TooldirError::invalid(format!("{TOOL_TOML}: [tool] {key} is required (a string)"))),
        }
    };
    let name = str_key("name")?;
    let kind = str_key("kind")?;
    let mut spec = match doc.get("spec") {
        None => Map::new(),
        Some(v) => match toml_to_json(v.clone())? {
            Value::Object(m) => m,
            _ => return Err(TooldirError::invalid(format!("{TOOL_TOML}: [spec] must be a table"))),
        },
    };
    if let Some(desc) = tool.get("description") {
        if spec.contains_key("description") {
            return Err(TooldirError::invalid(format!(
                "{TOOL_TOML}: description is set in both [tool] and [spec]; keep one"
            )));
        }
        spec.insert("description".into(), toml_to_json(desc.clone())?);
    }
    match file_field(&kind) {
        Some(FileField::Text(field, file)) => {
            if let Ok(bytes) = std::fs::read(dir.join(file)) {
                if spec.contains_key(field) {
                    return Err(TooldirError::invalid(format!(
                        "{TOOL_TOML}: [spec] {field} conflicts with {file}; keep one"
                    )));
                }
                let s = String::from_utf8(bytes)
                    .map_err(|_| TooldirError::invalid(format!("{file}: not valid UTF-8")))?;
                spec.insert(field.into(), Value::String(s));
            }
        }
        Some(FileField::Base64(field, file)) => {
            if let Ok(bytes) = std::fs::read(dir.join(file)) {
                if spec.contains_key(field) {
                    return Err(TooldirError::invalid(format!(
                        "{TOOL_TOML}: [spec] {field} conflicts with {file}; keep one"
                    )));
                }
                let b64 = base64::engine::general_purpose::STANDARD.encode(bytes);
                spec.insert(field.into(), Value::String(b64));
            }
        }
        None => {}
    }
    let mut payload = Map::new();
    payload.insert("name".into(), json!(name));
    payload.insert("kind".into(), json!(kind));
    payload.insert("spec".into(), Value::Object(spec));
    for key in EXTRA_TOOL_KEYS {
        if let Some(v) = tool.get(*key) {
            payload.insert((*key).into(), toml_to_json(v.clone())?);
        }
    }
    for key in tool.keys() {
        if !["name", "kind", "description"].contains(&key.as_str()) && !EXTRA_TOOL_KEYS.contains(&key.as_str()) {
            return Err(TooldirError::invalid(format!(
                "{TOOL_TOML}: [tool] has no field '{key}'; known: name, kind, description, {}",
                EXTRA_TOOL_KEYS.join(", ")
            )));
        }
    }
    Ok(canonical(&Value::Object(payload)))
}

/// Recursively key-sorted copy, so output order never depends on which
/// serde_json features the build unified.
pub fn canonical(value: &Value) -> Value {
    match value {
        Value::Object(m) => {
            let sorted: std::collections::BTreeMap<_, _> = m.iter().map(|(k, v)| (k.clone(), canonical(v))).collect();
            Value::Object(sorted.into_iter().collect())
        }
        Value::Array(a) => Value::Array(a.iter().map(canonical).collect()),
        other => other.clone(),
    }
}

/// Writes `payload` to `dir` and returns the files written. A payload whose
/// `kind` the registry does not know fails `unknown_kind` naming the
/// registry's names, and nothing is written; likewise any shape failure.
pub fn unpack(payload: &Value, dir: &Path, registry: &KindRegistry) -> Result<Vec<PathBuf>, TooldirError> {
    let obj = payload
        .as_object()
        .ok_or_else(|| TooldirError::invalid("payload: must be a JSON object {name, kind, spec}"))?;
    let kind = obj
        .get("kind")
        .and_then(Value::as_str)
        .ok_or_else(|| TooldirError::invalid("payload: kind is required (a string)"))?;
    let names = registry.names();
    if !names.contains(&kind) {
        return Err(TooldirError {
            code: crate::errors::UNKNOWN_KIND,
            message: format!("unknown_kind: '{kind}' is not a registered kind; registered: {}", names.join(", ")),
            error_code: None,
            data: json!({"kind": kind, "registered": names}),
        });
    }
    let name = obj
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| TooldirError::invalid("payload: name is required (a string)"))?;
    let mut spec = match obj.get("spec") {
        Some(Value::Object(m)) => m.clone(),
        _ => return Err(TooldirError::invalid("payload: spec is required (an object)")),
    };
    let mut files: Vec<(&str, Vec<u8>)> = Vec::new();
    match file_field(kind) {
        Some(FileField::Text(field, file)) => {
            if let Some(v) = spec.get(field) {
                let s = v
                    .as_str()
                    .ok_or_else(|| TooldirError::invalid(format!("spec.{field}: must be a string")))?;
                files.push((file, s.as_bytes().to_vec()));
                spec.remove(field);
            }
        }
        Some(FileField::Base64(field, file)) => {
            if let Some(v) = spec.get(field) {
                let s = v
                    .as_str()
                    .ok_or_else(|| TooldirError::invalid(format!("spec.{field}: must be a base64 string")))?;
                let bytes = base64::engine::general_purpose::STANDARD
                    .decode(s)
                    .map_err(|e| TooldirError::invalid(format!("spec.{field}: not valid base64: {e}")))?;
                // Only a canonical encoding round-trips byte-for-byte.
                if base64::engine::general_purpose::STANDARD.encode(&bytes) != s {
                    return Err(TooldirError::invalid(format!("spec.{field}: base64 is not canonical")));
                }
                files.push((file, bytes));
                spec.remove(field);
            }
        }
        None => {}
    }
    let mut tool = toml::Table::new();
    tool.insert("name".into(), toml::Value::String(name.to_string()));
    tool.insert("kind".into(), toml::Value::String(kind.to_string()));
    if let Some(Value::String(d)) = spec.get("description") {
        tool.insert("description".into(), toml::Value::String(d.clone()));
        spec.remove("description");
    }
    for key in obj.keys() {
        if ["name", "kind", "spec"].contains(&key.as_str()) {
            continue;
        }
        if !EXTRA_TOOL_KEYS.contains(&key.as_str()) {
            return Err(TooldirError::invalid(format!(
                "payload: '{key}' has no place in a tool directory; known: name, kind, spec, {}",
                EXTRA_TOOL_KEYS.join(", ")
            )));
        }
        tool.insert(key.clone(), json_to_toml(&obj[key])?);
    }
    let mut head = toml::Table::new();
    head.insert("tool".into(), toml::Value::Table(tool));
    let mut toml_text = toml::to_string(&head).map_err(|e| TooldirError::invalid(format!("{TOOL_TOML}: {e}")))?;
    if !spec.is_empty() {
        let spec_toml = match json_to_toml(&Value::Object(spec))? {
            toml::Value::Table(t) => t,
            _ => unreachable!("an object converts to a table"),
        };
        let mut wrap = toml::Table::new();
        wrap.insert("spec".into(), toml::Value::Table(spec_toml));
        toml_text.push('\n');
        toml_text.push_str(&toml::to_string(&wrap).map_err(|e| TooldirError::invalid(format!("{TOOL_TOML}: {e}")))?);
    }
    std::fs::create_dir_all(dir).map_err(|e| TooldirError::invalid(format!("{}: {e}", dir.display())))?;
    let mut written = Vec::new();
    files.insert(0, (TOOL_TOML, toml_text.into_bytes()));
    for (file, bytes) in files {
        let path = dir.join(file);
        std::fs::write(&path, bytes).map_err(|e| TooldirError::invalid(format!("{}: {e}", path.display())))?;
        written.push(path);
    }
    Ok(written)
}

/// Packs `dir`, then runs the server's own checks over the result:
/// `check_unknown_spec_field`, then the kind's `validate_all`. The first
/// finding is returned with the code and message the server would return.
pub fn lint(dir: &Path, registry: &KindRegistry) -> Result<(), TooldirError> {
    let payload = pack(dir)?;
    let kind_name = payload["kind"].as_str().unwrap_or_default();
    let Some(kind) = registry.get(kind_name).filter(|_| registry.names().contains(&kind_name)) else {
        let names = registry.names();
        return Err(TooldirError {
            code: crate::errors::UNKNOWN_KIND,
            message: format!("unknown_kind: '{kind_name}' is not a registered kind; registered: {}", names.join(", ")),
            error_code: None,
            data: json!({"kind": kind_name, "registered": names}),
        });
    };
    if let Some(err) = check_unknown_spec_field(kind.as_ref(), &payload["spec"], registry) {
        return Err(from_kind_error(&err));
    }
    if let Some(err) = kind.validate_all(&payload["spec"]).into_iter().next() {
        return Err(from_kind_error(&err));
    }
    lint_tests(dir)
}

fn from_kind_error(err: &crate::kinds::KindError) -> TooldirError {
    use crate::kinds::KindError;
    match err {
        KindError::Structured { code, message, data } => TooldirError {
            code: crate::errors::INVALID_SPEC,
            message: message.clone(),
            error_code: Some(code),
            data: data.clone(),
        },
        KindError::InvalidSpec(m) => TooldirError::invalid(m.clone()),
        other => TooldirError::invalid(other.to_string()),
    }
}

/// `tests/<name>.json` must each be an object with an `args` object (the
/// `host.spec_test` `invocations` shape) and an optional `expect`.
fn lint_tests(dir: &Path) -> Result<(), TooldirError> {
    for path in test_files(dir) {
        let rel = format!("{TESTS_DIR}/{}", path.file_name().and_then(|n| n.to_str()).unwrap_or("?"));
        let text = std::fs::read_to_string(&path).map_err(|e| TooldirError::invalid(format!("{rel}: {e}")))?;
        let v: Value =
            serde_json::from_str(&text).map_err(|e| TooldirError::invalid(format!("{rel}: not valid JSON: {e}")))?;
        let Some(o) = v.as_object() else {
            return Err(TooldirError::invalid(format!("{rel}: must be an object {{args, expect?}}")));
        };
        if !o.get("args").is_some_and(Value::is_object) {
            return Err(TooldirError::invalid(format!("{rel}: args is required (an object)")));
        }
        if let Some(key) = o.keys().find(|k| !["args", "expect"].contains(&k.as_str())) {
            return Err(TooldirError::invalid(format!("{rel}: '{key}' is not a test field; known: args, expect")));
        }
    }
    Ok(())
}

/// `dir/tests/*.json`, sorted by name.
pub fn test_files(dir: &Path) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = std::fs::read_dir(dir.join(TESTS_DIR))
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .collect();
    out.sort();
    out
}

/// PRD R6: the payload wrapped as a JSON-RPC `tools/call` body for
/// `host.tool_publish`, ready to POST to a bare `/mcp`.
pub fn as_mcp_call(payload: &Value) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": {
            "name": "host.tool_publish",
            "arguments": payload,
            // SEP-2575: the host runs every request stateless, so a bare
            // `curl` has no `initialize` to carry this client context.
            "_meta": {
                "io.modelcontextprotocol/protocolVersion": "2026-07-28",
                "io.modelcontextprotocol/clientCapabilities": {},
                "io.modelcontextprotocol/clientInfo": {"name": "mcphost-tool-pack", "version": env!("CARGO_PKG_VERSION")},
            },
        },
    })
}

/// The generated `docs/tool-directory.md` (and the llms.txt "Tool
/// directories" block): layout, then the payload field table from
/// `tool_publish_props`, then each kind's spec fields from
/// `known_spec_fields` plus the universal ones.
pub fn render_docs(registry: &KindRegistry) -> String {
    let mut out = String::new();
    out.push_str("<!-- GENERATED by `mcphost gen-docs --tool-directory`; do not edit by hand. -->\n");
    out.push_str("# Tool directories\n\n");
    out.push_str(&render_block(registry));
    out
}

/// The body shared by the doc page and the `www/llms.txt` section.
pub fn render_block(registry: &KindRegistry) -> String {
    let mut out = String::new();
    out.push_str("A tool is a directory; `mcphost tool pack <dir>` emits the `host.tool_publish` payload.\n\n");
    out.push_str("| file | holds |\n|---|---|\n");
    out.push_str("| `tool.toml` | `[tool]` name, kind, description, and `[spec]` (every spec field not kept in its own file) |\n");
    out.push_str("| `tool.py` | a python tool's `spec.source` |\n");
    out.push_str("| `tool.wasm` | a wasm tool's raw `spec.component` bytes (packed as base64) |\n");
    out.push_str("| `tests/<name>.json` | `{args, expect?}` invocations; never part of the payload |\n\n");
    out.push_str("`mcphost tool pack <dir> --as-mcp-call` wraps the payload in a JSON-RPC `tools/call` body for `host.tool_publish`; POST it to a bare `/mcp` with the headers `content-type: application/json`, `MCP-Protocol-Version: 2026-07-28`, `Mcp-Method: tools/call` and `Mcp-Name: host.tool_publish` (implicit signup mints the tenant).\n\n");
    out.push_str("## Publish payload fields\n\n| field | type | required |\n|---|---|---|\n");
    let props = crate::handler::tool_publish_props(registry);
    if let Some(map) = props.as_object() {
        for (name, def) in map {
            let ty = def.get("type").and_then(Value::as_str).unwrap_or("any");
            let required = ["name", "kind", "spec"].contains(&name.as_str());
            out.push_str(&format!("| `{name}` | {ty} | {} |\n", if required { "yes" } else { "no" }));
        }
    }
    out.push_str("\nUniversal spec fields (every kind): ");
    out.push_str(&UNIVERSAL_SPEC_FIELDS.iter().map(|f| format!("`{f}`")).collect::<Vec<_>>().join(", "));
    out.push_str("\n\n## Spec fields by kind\n\n| kind | required | file | fields |\n|---|---|---|---|\n");
    for name in registry.names() {
        let Some(kind) = registry.get(name) else { continue };
        let required = kind.required_spec_field().map_or_else(|| "-".to_string(), |f| format!("`{f}`"));
        let file = match file_field(name) {
            Some(FileField::Text(f, file)) | Some(FileField::Base64(f, file)) => format!("`{file}` = `{f}`"),
            None => "-".to_string(),
        };
        let mut fields: Vec<&str> = kind.known_spec_fields().to_vec();
        fields.sort_unstable();
        let fields = fields.iter().map(|f| format!("`{f}`")).collect::<Vec<_>>().join(", ");
        out.push_str(&format!("| {name} | {required} | {file} | {fields} |\n"));
    }
    out
}

/// Compares `committed` to the fresh render; on drift names the first
/// differing line (1-based) with both sides.
pub fn check_doc(registry: &KindRegistry, committed: &str) -> Result<(), String> {
    let fresh = render_docs(registry);
    if fresh == committed {
        return Ok(());
    }
    let mut a = fresh.lines();
    let mut b = committed.lines();
    let mut n = 0;
    loop {
        n += 1;
        match (a.next(), b.next()) {
            (Some(x), Some(y)) if x == y => continue,
            (x, y) => {
                return Err(format!(
                    "{DOC_PATH} line {n} differs: generated {:?}, committed {:?}; run `mcphost gen-docs --tool-directory`",
                    x.unwrap_or("<end of file>"),
                    y.unwrap_or("<end of file>")
                ));
            }
        }
    }
}
