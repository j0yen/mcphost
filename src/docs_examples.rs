//! PRD-mcphost-docs-examples-executed-nightly: every tagged fence in the
//! docs is an executable example.
//!
//! This module is the ONLY parser of the fence tag grammar (PRD technical
//! considerations): `mcphost-call` / `mcphost-tool` plus the `expect=`,
//! `needs=`, `skip=`, `kind=`, `name=` attributes, all on the fence's info
//! string. Everything else (`classify`, `extract`, `run`, the llms-full
//! annotator) reads fences through [`parse_fences`].

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Info-string tag for a JSON-RPC call fence.
pub const TAG_CALL: &str = "mcphost-call";
/// Info-string tag for a tool source / spec fence.
pub const TAG_TOOL: &str = "mcphost-tool";

/// One fenced code block as it appears in a markdown page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fence {
    /// 1-based line of the opening fence.
    pub line: usize,
    /// Slug of the nearest preceding markdown heading (`page` if none).
    pub heading: String,
    /// The raw info string after the backticks.
    pub info: String,
    /// The fence body, with the opening fence's indentation stripped.
    pub body: String,
    /// `Some(line)` of the closing fence; `None` for the last fence of an
    /// unterminated page.
    pub close_line: Option<usize>,
}

/// The parsed info string of a [`Fence`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Info {
    pub lang: Option<String>,
    /// `mcphost-call` / `mcphost-tool` when the fence carries a tag.
    pub tag: Option<String>,
    pub attrs: BTreeMap<String, String>,
}

impl Info {
    pub fn parse(info: &str) -> Info {
        let mut out = Info::default();
        for (i, tok) in info.split_whitespace().enumerate() {
            if tok == TAG_CALL || tok == TAG_TOOL {
                out.tag = Some(tok.to_string());
            } else if let Some((k, v)) = tok.split_once('=') {
                out.attrs.insert(k.to_string(), v.to_string());
            } else if i == 0 {
                out.lang = Some(tok.to_string());
            }
        }
        out
    }
}

fn heading_slug(text: &str) -> String {
    let mut out = String::new();
    let mut pending_dash = false;
    for c in text.chars() {
        if c.is_ascii_alphanumeric() {
            if pending_dash && !out.is_empty() {
                out.push('-');
            }
            pending_dash = false;
            out.push(c.to_ascii_lowercase());
        } else {
            pending_dash = true;
        }
    }
    if out.is_empty() { "page".to_string() } else { out }
}

/// `(indent, fence char, fence length, rest)` when `line` opens or closes a
/// fence.
fn fence_marker(line: &str) -> Option<(usize, char, usize, &str)> {
    let trimmed = line.trim_start_matches(' ');
    let indent = line.len() - trimmed.len();
    let c = trimmed.chars().next()?;
    if c != '`' && c != '~' {
        return None;
    }
    let n = trimmed.chars().take_while(|x| *x == c).count();
    if n < 3 {
        return None;
    }
    Some((indent, c, n, &trimmed[n..]))
}

/// Every fenced block of `text`, in page order.
pub fn parse_fences(text: &str) -> Vec<Fence> {
    let mut out = Vec::new();
    let mut heading = "page".to_string();
    let mut open: Option<(usize, usize, char, usize, String, String, Vec<String>)> = None;
    for (i, raw) in text.lines().enumerate() {
        let lineno = i + 1;
        if let Some((start, indent, c, n, info, hd, body)) = open.as_mut() {
            let closes = fence_marker(raw)
                .is_some_and(|(_, c2, n2, rest)| c2 == *c && n2 >= *n && rest.trim().is_empty());
            if closes {
                out.push(Fence {
                    line: *start,
                    heading: hd.clone(),
                    info: info.clone(),
                    body: body.join("\n"),
                    close_line: Some(lineno),
                });
                open = None;
            } else {
                let strip = raw.len() - raw.trim_start_matches(' ').len();
                body.push(raw[strip.min(*indent)..].to_string());
            }
            continue;
        }
        if let Some((indent, c, n, rest)) = fence_marker(raw) {
            open = Some((lineno, indent, c, n, rest.trim().to_string(), heading.clone(), Vec::new()));
            continue;
        }
        let t = raw.trim_start();
        if t.starts_with('#') {
            let hashes = t.chars().take_while(|x| *x == '#').count();
            if hashes <= 6 && t[hashes..].starts_with(' ') {
                heading = heading_slug(t[hashes..].trim());
            }
        }
    }
    if let Some((start, _, _, _, info, hd, body)) = open {
        out.push(Fence { line: start, heading: hd, info, body: body.join("\n"), close_line: None });
    }
    out
}

/// What a fence is, from the viewpoint of an agent copying it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Class {
    /// A complete call, command, spec or argument payload, runnable as
    /// written.
    Runnable,
    /// Looks like input but carries a placeholder, an ellipsis or an
    /// environment dependency, so it cannot run as written.
    Fragment,
    /// Output samples, transcripts and other non-input text.
    Prose,
}

impl Class {
    pub fn as_str(self) -> &'static str {
        match self {
            Class::Runnable => "runnable",
            Class::Fragment => "fragment",
            Class::Prose => "prose",
        }
    }
}

/// `<placeholder>` (angle brackets around a word), or an ellipsis.
fn has_placeholder(body: &str) -> bool {
    if body.contains("...") {
        return true;
    }
    let bytes = body.as_bytes();
    for (i, b) in bytes.iter().enumerate() {
        if *b == b'<' && bytes.get(i + 1).is_some_and(|n| n.is_ascii_alphabetic()) {
            let rest = &body[i + 1..];
            let end = rest.find(['>', '<', '\n']);
            if end.is_some_and(|e| rest.as_bytes()[e] == b'>') {
                return true;
            }
        }
    }
    false
}

/// A `$VAR` / `${VAR}` reference: the fence depends on the reader's shell.
fn has_env_reference(body: &str) -> bool {
    let bytes = body.as_bytes();
    bytes.iter().enumerate().any(|(i, b)| {
        *b == b'$' && bytes.get(i + 1).is_some_and(|n| n.is_ascii_alphabetic() || *n == b'{' || *n == b'_')
    })
}

const INPUT_KEYS: &[&str] =
    &["source", "component", "method", "schema", "steps", "mcpServers", "name", "arguments", "jsonrpc"];

const COMMANDS: &[&str] = &[
    "claude", "codex", "gemini", "opencode", "amp", "cargo", "cp", "systemctl", "curl", "cd", "npx", "uvx", "docker",
];

fn is_call_line(line: &str) -> bool {
    let t = line.trim();
    let Some(open) = t.find('(') else { return false };
    let name = &t[..open];
    !name.is_empty()
        && name.chars().next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.')
        && t.ends_with(')')
}

/// Classify one fence. A tag decides outright; untagged fences are judged by
/// their content so `classify` measures the docs as written, before (and
/// independent of) any tagging.
pub fn classify(fence: &Fence) -> Class {
    let info = Info::parse(&fence.info);
    if info.tag.is_some() {
        return if info.attrs.get("skip").is_some_and(|s| s == "fragment") {
            Class::Fragment
        } else {
            Class::Runnable
        };
    }
    let body = fence.body.trim();
    if body.is_empty() {
        return Class::Prose;
    }
    let lang = info.lang.as_deref().unwrap_or("");
    let looks_json = lang == "json" || body.starts_with('{');
    if looks_json {
        if has_placeholder(body) {
            return Class::Fragment;
        }
        let Ok(Value::Object(map)) = serde_json::from_str::<Value>(body) else {
            return Class::Prose;
        };
        if map.keys().any(|k| k.starts_with('_')) {
            return Class::Prose;
        }
        if map.keys().any(|k| INPUT_KEYS.contains(&k.as_str())) {
            return Class::Runnable;
        }
        let flat = map.values().all(|v| !v.is_object() && !v.is_array());
        return if flat && map.len() <= 3 { Class::Runnable } else { Class::Prose };
    }
    let lines: Vec<&str> = body.lines().filter(|l| !l.trim().is_empty()).collect();
    if lines.iter().all(|l| is_call_line(l)) {
        return if has_placeholder(body) { Class::Fragment } else { Class::Runnable };
    }
    if lines.first().is_some_and(|l| l.trim_start().starts_with("$ ")) {
        return Class::Prose;
    }
    let first_cmd = lines
        .iter()
        .map(|l| l.trim())
        .find(|l| !l.starts_with('#'))
        .and_then(|l| l.split_whitespace().next())
        .unwrap_or("");
    let is_command = COMMANDS.contains(&first_cmd) || first_cmd.starts_with("./") || first_cmd.starts_with("examples/");
    if !is_command {
        return Class::Prose;
    }
    if has_placeholder(body) || has_env_reference(body) {
        return Class::Fragment;
    }
    Class::Runnable
}

/// Result of `docs-examples classify`.
#[derive(Debug, Clone, Default)]
pub struct ClassifyReport {
    /// One `<path>:<line> <class>` per fence, in page order.
    pub lines: Vec<String>,
    pub runnable: usize,
    pub fragment: usize,
    pub prose: usize,
    /// `<path>:<line>` of every fence that is not runnable.
    pub non_runnable: Vec<String>,
}

impl ClassifyReport {
    pub fn total(&self) -> usize {
        self.runnable + self.fragment + self.prose
    }
}

pub fn classify_text(path: &str, text: &str) -> ClassifyReport {
    let mut r = ClassifyReport::default();
    for f in parse_fences(text) {
        let class = classify(&f);
        match class {
            Class::Runnable => r.runnable += 1,
            Class::Fragment => r.fragment += 1,
            Class::Prose => r.prose += 1,
        }
        let at = format!("{path}:{}", f.line);
        r.lines.push(format!("{at} {}", class.as_str()));
        if class != Class::Runnable {
            r.non_runnable.push(format!("{at} {}", class.as_str()));
        }
    }
    r
}

/// One entry of `docs/examples.corpus.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    pub id: String,
    pub tag: String,
    pub attrs: BTreeMap<String, String>,
    pub body: String,
    pub expect: Option<String>,
}

/// Default corpus location, relative to the repo root.
pub const CORPUS_PATH: &str = "docs/examples.corpus.json";

fn collect_md(dir: &std::path::Path, rel: &str, out: &mut Vec<String>) -> std::io::Result<()> {
    let Ok(rd) = std::fs::read_dir(dir) else { return Ok(()) };
    let mut names: Vec<_> = rd.filter_map(Result::ok).collect();
    names.sort_by_key(|e| e.file_name());
    for e in names {
        let name = e.file_name().to_string_lossy().to_string();
        let rel_path = format!("{rel}/{name}");
        if e.file_type()?.is_dir() {
            collect_md(&e.path(), &rel_path, out)?;
        } else if name.ends_with(".md") {
            out.push(rel_path);
        }
    }
    Ok(())
}

/// Every markdown page the extractor scans: `docs/**/*.md` under `root`,
/// sorted by path. (`README.md` is excluded on purpose: its quickstart
/// block is a generated copy of `docs/agent-quickstart.md`.)
pub fn corpus_pages(root: &std::path::Path) -> std::io::Result<Vec<String>> {
    let mut pages = Vec::new();
    collect_md(&root.join("docs"), "docs", &mut pages)?;
    pages.sort();
    Ok(pages)
}

fn valid_expect(e: &str) -> bool {
    e == "ok"
        || e.strip_prefix("error:").is_some_and(|c| !c.is_empty())
        || e.strip_prefix("contains:").is_some_and(|c| !c.is_empty())
}

/// Entries of one page, plus tag-grammar errors.
pub fn extract_page(path: &str, text: &str, errors: &mut Vec<String>) -> Vec<Entry> {
    let mut counters: BTreeMap<String, usize> = BTreeMap::new();
    let mut out = Vec::new();
    for f in parse_fences(text) {
        let info = Info::parse(&f.info);
        let Some(tag) = info.tag.clone() else { continue };
        let n = counters.entry(f.heading.clone()).or_insert(0);
        *n += 1;
        let id = format!("{path}#{}-{n}", f.heading);
        let mut bad = |msg: String| errors.push(format!("{id}: {msg}"));
        if let Some(e) = info.attrs.get("expect")
            && !valid_expect(e)
        {
            bad(format!("expect={e} is not ok|error:<code>|contains:<text>"));
        }
        if let Some(v) = info.attrs.get("needs")
            && v != "tenant"
            && v != "anonymous"
        {
            bad(format!("needs={v} is not tenant|anonymous"));
        }
        if tag == TAG_TOOL && !info.attrs.contains_key("name") {
            bad("mcphost-tool requires name=<tool name>".to_string());
        }
        if tag == TAG_TOOL && !info.attrs.contains_key("kind") {
            bad("mcphost-tool requires kind=<kind>".to_string());
        }
        out.push(Entry {
            id,
            tag,
            expect: info.attrs.get("expect").cloned(),
            attrs: info.attrs,
            body: f.body,
        });
    }
    out
}

/// Extract the whole corpus under `root`: `Ok(entries)` or the list of
/// errors that make the corpus invalid.
pub fn extract(root: &std::path::Path) -> Result<Vec<Entry>, Vec<String>> {
    let pages = corpus_pages(root).map_err(|e| vec![format!("scan docs/: {e}")])?;
    let mut errors = Vec::new();
    let mut entries = Vec::new();
    // `pages` is sorted and each page is extracted in fence order, so the
    // corpus is already sorted by path then order.
    for page in &pages {
        let text = std::fs::read_to_string(root.join(page)).map_err(|e| vec![format!("read {page}: {e}")])?;
        entries.extend(extract_page(page, &text, &mut errors));
    }
    if errors.is_empty() { Ok(entries) } else { Err(errors) }
}

/// The page-path half of an id.
pub fn page_of(id: &str) -> &str {
    id.split_once('#').map_or(id, |(p, _)| p)
}

/// The canonical on-disk form of a corpus.
pub fn render_corpus(entries: &[Entry]) -> String {
    let mut s = serde_json::to_string_pretty(entries).expect("corpus serializes");
    s.push('\n');
    s
}

/// Ids whose entry differs between `committed` and `fresh` (added, removed
/// or changed), in a stable order.
pub fn corpus_drift(committed: &[Entry], fresh: &[Entry]) -> Vec<String> {
    let old: BTreeMap<&str, &Entry> = committed.iter().map(|e| (e.id.as_str(), e)).collect();
    let new: BTreeMap<&str, &Entry> = fresh.iter().map(|e| (e.id.as_str(), e)).collect();
    let mut ids: Vec<String> = Vec::new();
    for (id, e) in &new {
        if old.get(id) != Some(e) {
            ids.push((*id).to_string());
        }
    }
    for id in old.keys() {
        if !new.contains_key(id) {
            ids.push((*id).to_string());
        }
    }
    ids
}

pub fn load_corpus(path: &std::path::Path) -> Result<Vec<Entry>, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("read {}: {e}", path.display()))?;
    serde_json::from_str(&text).map_err(|e| format!("parse {}: {e}", path.display()))
}

// ---------------------------------------------------------------- run ----

/// Split `"name(k=v, ...)"` into the callee and the raw argument text.
fn split_call(body: &str) -> Option<(&str, &str)> {
    let t = body.trim();
    let open = t.find('(')?;
    let inner = t.strip_suffix(')')?;
    Some((t[..open].trim(), &inner[open + 1..]))
}

/// Parse `k=<json>, k2=<json>` (python-style keyword arguments whose values
/// are JSON literals, the form every `host.*(...)` example in the docs
/// uses) into an object.
fn parse_kwargs(args: &str) -> Result<serde_json::Map<String, Value>, String> {
    let mut map = serde_json::Map::new();
    let mut rest = args.trim();
    while !rest.is_empty() {
        let eq = rest.find('=').ok_or_else(|| format!("expected key=value in `{rest}`"))?;
        let key = rest[..eq].trim();
        if key.is_empty() || !key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            return Err(format!("bad argument name `{key}`"));
        }
        let mut stream = serde_json::Deserializer::from_str(&rest[eq + 1..]).into_iter::<Value>();
        let value = stream
            .next()
            .ok_or_else(|| format!("no value for `{key}`"))?
            .map_err(|e| format!("value of `{key}` is not JSON: {e}"))?;
        let consumed = stream.byte_offset();
        map.insert(key.to_string(), value);
        rest = rest[eq + 1 + consumed..].trim_start();
        rest = rest.strip_prefix(',').unwrap_or(rest).trim_start();
    }
    Ok(map)
}

/// A `mcphost-call` body as `(method, params)`: a `{name, arguments}`
/// object (the `tools/call` params), a full JSON-RPC request object, or the
/// docs' `tool_name(key=value, ...)` shorthand.
pub fn parse_call(body: &str) -> Result<(String, Value), String> {
    let t = body.trim();
    if t.starts_with('{') {
        let v: Value = serde_json::from_str(t).map_err(|e| format!("call body is not JSON: {e}"))?;
        if let Some(method) = v.get("method").and_then(Value::as_str) {
            return Ok((method.to_string(), v.get("params").cloned().unwrap_or_else(|| serde_json::json!({}))));
        }
        let name = v.get("name").and_then(Value::as_str).ok_or("call body has no `name`")?;
        let arguments = v.get("arguments").cloned().unwrap_or_else(|| serde_json::json!({}));
        return Ok(("tools/call".to_string(), serde_json::json!({"name": name, "arguments": arguments})));
    }
    let (callee, raw) = split_call(t).ok_or("call body is neither JSON nor `tool(args)`")?;
    let arguments = parse_kwargs(raw)?;
    Ok(("tools/call".to_string(), serde_json::json!({"name": callee, "arguments": Value::Object(arguments)})))
}

/// A `mcphost-tool` body as the publish `spec`: a JSON object as written, or
/// (for `kind=python`) bare python source.
pub fn tool_spec(kind: &str, body: &str) -> Result<Value, String> {
    let t = body.trim();
    if t.starts_with('{')
        && let Ok(v @ Value::Object(_)) = serde_json::from_str::<Value>(t)
    {
        return Ok(v);
    }
    if kind == "python" {
        return Ok(serde_json::json!({"source": body}));
    }
    Err(format!("kind={kind} spec body must be a JSON object"))
}

/// What one call came back with, reduced to what `expect=` can test.
#[derive(Debug, Clone)]
pub struct Outcome {
    pub ok: bool,
    /// The error code when `ok` is false.
    pub code: Option<String>,
    /// `result.content[0].text` (the `contains:` target); the whole result
    /// as JSON when there is no text content.
    pub text: String,
}

impl Outcome {
    pub fn actual(&self) -> String {
        if self.ok { "ok".to_string() } else { format!("error:{}", self.code.as_deref().unwrap_or("unknown")) }
    }
}

/// `Ok(())` when `outcome` satisfies `expect` (default `ok`), else the
/// actual result in `expect` vocabulary.
pub fn evaluate(expect: Option<&str>, outcome: &Outcome) -> Result<(), String> {
    let expect = expect.unwrap_or("ok");
    let pass = if expect == "ok" {
        outcome.ok
    } else if let Some(code) = expect.strip_prefix("error:") {
        !outcome.ok && outcome.code.as_deref() == Some(code)
    } else if let Some(text) = expect.strip_prefix("contains:") {
        outcome.ok && outcome.text.contains(text)
    } else {
        false
    };
    if pass { Ok(()) } else { Err(format!("expected={expect} actual={}", outcome.actual())) }
}

/// A tool result's structured payload: `structuredContent`, else the JSON
/// in `content[0].text`, else null.
fn structured_of(result: &Value) -> Value {
    if let Some(sc) = result.get("structuredContent") {
        return sc.clone();
    }
    result
        .get("content")
        .and_then(|c| c.get(0))
        .and_then(|c| c.get("text"))
        .and_then(Value::as_str)
        .and_then(|t| serde_json::from_str(t).ok())
        .unwrap_or(Value::Null)
}

fn outcome_of(result: Result<Value, CallError>) -> Result<Outcome, String> {
    match result {
        Err(CallError::Transport(e)) => Err(e),
        Err(CallError::Rpc { code }) => Ok(Outcome { ok: false, code: Some(code), text: String::new() }),
        Ok(result) => {
            let text = result
                .get("content")
                .and_then(Value::as_array)
                .and_then(|c| c.first())
                .and_then(|c| c.get("text"))
                .and_then(Value::as_str)
                .map_or_else(|| result.to_string(), str::to_string);
            if result.get("isError").and_then(Value::as_bool) == Some(true) {
                let structured = structured_of(&result);
                let code = structured
                    .get("error_code")
                    .or_else(|| structured.get("code"))
                    .and_then(Value::as_str)
                    .unwrap_or("tool_error")
                    .to_string();
                return Ok(Outcome { ok: false, code: Some(code), text });
            }
            Ok(Outcome { ok: true, code: None, text })
        }
    }
}

#[derive(Debug)]
enum CallError {
    /// The request never got an HTTP answer.
    Transport(String),
    /// A JSON-RPC error; `code` is `data.error_code` when present.
    Rpc { code: String },
}

struct Rpc {
    http: reqwest::Client,
    endpoint: String,
    next_id: u64,
}

impl Rpc {
    async fn call(&mut self, bearer: Option<&str>, method: &str, mut params: Value) -> Result<Value, CallError> {
        self.next_id += 1;
        if let Some(obj) = params.as_object_mut() {
            obj.insert(
                "_meta".to_string(),
                serde_json::json!({
                    "io.modelcontextprotocol/protocolVersion": "2026-07-28",
                    "io.modelcontextprotocol/clientCapabilities": {},
                    "io.modelcontextprotocol/clientInfo": {"name": "mcphost-docs-examples", "version": env!("CARGO_PKG_VERSION")},
                }),
            );
        }
        let body = serde_json::json!({"jsonrpc": "2.0", "id": self.next_id, "method": method, "params": params});
        let mut req = self
            .http
            .post(&self.endpoint)
            .header("Content-Type", "application/json")
            .header("Accept", "application/json, text/event-stream")
            .header("MCP-Protocol-Version", "2026-07-28")
            .header("Mcp-Method", method);
        if method == "tools/call"
            && let Some(name) = params.get("name").and_then(Value::as_str)
        {
            req = req.header("Mcp-Name", name);
        }
        if let Some(key) = bearer {
            req = req.header("Authorization", format!("Bearer {key}"));
        }
        let resp = req.json(&body).send().await.map_err(|e| CallError::Transport(e.to_string()))?;
        let is_sse = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|c| c.starts_with("text/event-stream"));
        let status = resp.status();
        let text = resp.text().await.map_err(|e| CallError::Transport(e.to_string()))?;
        let message: Value = if is_sse {
            text.lines()
                .filter_map(|l| l.strip_prefix("data:"))
                .filter_map(|d| serde_json::from_str::<Value>(d.trim()).ok())
                .next_back()
                .ok_or_else(|| CallError::Transport(format!("empty event stream ({status})")))?
        } else {
            serde_json::from_str(&text).map_err(|e| CallError::Transport(format!("HTTP {status}, body is not JSON: {e}")))?
        };
        if let Some(error) = message.get("error") {
            let code = error
                .get("data")
                .and_then(|d| d.get("error_code"))
                .and_then(Value::as_str)
                .map(str::to_string)
                .unwrap_or_else(|| format!("rpc_{}", error.get("code").and_then(Value::as_i64).unwrap_or(0)));
            return Err(CallError::Rpc { code });
        }
        Ok(message.get("result").cloned().unwrap_or(Value::Null))
    }
}

/// One executed example.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExampleResult {
    pub id: String,
    /// `pass`, `fail` or `skip`.
    pub status: String,
    pub ms: u64,
    pub detail: String,
}

/// The outcome of `docs-examples run`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Report {
    pub date: String,
    pub url: String,
    pub examples_total: usize,
    pub examples_failed: usize,
    pub skipped: usize,
    pub results: Vec<ExampleResult>,
}

impl Report {
    /// `<id> pass <ms>` per example, then the summary line.
    pub fn render_lines(&self) -> Vec<String> {
        let mut lines: Vec<String> = self
            .results
            .iter()
            .map(|r| match r.status.as_str() {
                "pass" => format!("{} pass {}", r.id, r.ms),
                "skip" => format!("{} skip {}", r.id, r.detail),
                _ => format!("{} fail {} {}", r.id, r.ms, r.detail),
            })
            .collect();
        lines.push(format!(
            "examples_total={} examples_failed={} skipped={}",
            self.examples_total, self.examples_failed, self.skipped
        ));
        lines
    }
}

/// `run` could not execute the corpus.
#[derive(Debug)]
pub enum RunError {
    /// Nothing answered at the URL, even after one retry.
    Unreachable(String),
    /// The throwaway tenant could not be created.
    Signup(String),
}

impl std::fmt::Display for RunError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RunError::Unreachable(e) => write!(f, "host unreachable: {e}"),
            RunError::Signup(e) => write!(f, "signup failed: {e}"),
        }
    }
}

pub struct RunOpts {
    /// The host's base URL, or its `/mcp` endpoint.
    pub url: String,
    /// Run as this tenant instead of signing up a throwaway one
    /// (`MCPHOST_TENANT_KEY`).
    pub tenant_key: Option<String>,
    pub retry_delay: std::time::Duration,
}

fn endpoint_of(url: &str) -> String {
    let u = url.trim_end_matches('/');
    if u.ends_with("/mcp") { u.to_string() } else { format!("{u}/mcp") }
}

/// `YYYY-MM-DD` (UTC) for a unix time.
pub fn civil_date(unix_secs: u64) -> String {
    let z = (unix_secs / 86_400) as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
}

fn today() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    civil_date(secs)
}

async fn run_entry(rpc: &mut Rpc, bearer: Option<&str>, entry: &Entry) -> Result<(), String> {
    let transport = |e: String| format!("transport error: {e}");
    if entry.tag == TAG_TOOL {
        let kind = entry.attrs.get("kind").cloned().unwrap_or_default();
        let name = entry.attrs.get("name").cloned().unwrap_or_default();
        let spec = tool_spec(&kind, &entry.body)?;
        let publish = outcome_of(
            rpc.call(
                bearer,
                "tools/call",
                serde_json::json!({"name": "host.tool_publish", "arguments": {"name": name, "kind": kind, "spec": spec}}),
            )
            .await,
        )
        .map_err(transport)?;
        // `expect` applies to the publish unless the fence asks for a
        // specific outcome of the test call.
        let test_args: Value = entry
            .attrs
            .get("args")
            .and_then(|a| serde_json::from_str(a).ok())
            .unwrap_or_else(|| serde_json::json!({}));
        let verdict = if entry.expect.is_some() && publish.ok {
            let test = outcome_of(
                rpc.call(
                    bearer,
                    "tools/call",
                    serde_json::json!({"name": "host.tool_test", "arguments": {"name": name, "args": test_args}}),
                )
                .await,
            )
            .map_err(transport)?;
            evaluate(entry.expect.as_deref(), &test)
        } else {
            let verdict = evaluate(entry.expect.as_deref(), &publish);
            if publish.ok {
                // The PRD's "publish then one host.tool_test": the smoke
                // call is made, its result is not part of the verdict.
                let _ = rpc
                    .call(
                        bearer,
                        "tools/call",
                        serde_json::json!({"name": "host.tool_test", "arguments": {"name": name, "args": test_args}}),
                    )
                    .await;
            }
            verdict
        };
        return verdict;
    }
    let (method, params) = parse_call(&entry.body)?;
    let outcome = outcome_of(rpc.call(bearer, &method, params).await).map_err(transport)?;
    evaluate(entry.expect.as_deref(), &outcome)
}

/// Execute `entries` in order against the host at `opts.url`.
pub async fn run(opts: &RunOpts, entries: &[Entry]) -> Result<Report, RunError> {
    let http = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(120))
        .build()
        .map_err(|e| RunError::Unreachable(e.to_string()))?;
    let mut rpc = Rpc { http, endpoint: endpoint_of(&opts.url), next_id: 0 };

    // Preflight: one anonymous `tools/list`, retried once. Only a transport
    // failure (nothing answered) means "host unreachable".
    let mut preflight = rpc.call(None, "tools/list", serde_json::json!({})).await;
    if matches!(preflight, Err(CallError::Transport(_))) {
        tokio::time::sleep(opts.retry_delay).await;
        preflight = rpc.call(None, "tools/list", serde_json::json!({})).await;
    }
    if let Err(CallError::Transport(e)) = preflight {
        return Err(RunError::Unreachable(e));
    }

    let (key, throwaway) = match &opts.tenant_key {
        Some(k) => (k.clone(), false),
        None => {
            let name = format!("docs-examples-{}", today());
            let result = rpc
                .call(None, "tools/call", serde_json::json!({"name": "signup", "arguments": {"name": name}}))
                .await
                .map_err(|e| RunError::Signup(format!("{e:?}")))?;
            let key = structured_of(&result)
                .get("key")
                .and_then(Value::as_str)
                .map(str::to_string)
                .ok_or_else(|| RunError::Signup("signup returned no key".to_string()))?;
            (key, true)
        }
    };

    let mut results = Vec::with_capacity(entries.len());
    for entry in entries {
        if let Some(reason) = entry.attrs.get("skip") {
            results.push(ExampleResult { id: entry.id.clone(), status: "skip".into(), ms: 0, detail: reason.clone() });
            continue;
        }
        let bearer = if entry.attrs.get("needs").is_some_and(|n| n == "anonymous") { None } else { Some(key.as_str()) };
        let started = std::time::Instant::now();
        let verdict = run_entry(&mut rpc, bearer, entry).await;
        let ms = started.elapsed().as_millis() as u64;
        results.push(match verdict {
            Ok(()) => ExampleResult { id: entry.id.clone(), status: "pass".into(), ms, detail: String::new() },
            Err(detail) => ExampleResult { id: entry.id.clone(), status: "fail".into(), ms, detail },
        });
    }

    if throwaway {
        let _ = rpc
            .call(Some(&key), "tools/call", serde_json::json!({"name": "host.self_offboard", "arguments": {}}))
            .await;
    }

    let failed = results.iter().filter(|r| r.status == "fail").count();
    let skipped = results.iter().filter(|r| r.status == "skip").count();
    Ok(Report {
        date: today(),
        url: opts.url.clone(),
        examples_total: results.len(),
        examples_failed: failed,
        skipped,
        results,
    })
}

/// Write `<dir>/<date>.json`; returns the path.
pub fn write_artifact(dir: &std::path::Path, report: &Report) -> std::io::Result<std::path::PathBuf> {
    std::fs::create_dir_all(dir)?;
    let path = dir.join(format!("{}.json", report.date));
    let mut text = serde_json::to_string_pretty(report).expect("report serializes");
    text.push('\n');
    std::fs::write(&path, text)?;
    Ok(path)
}
