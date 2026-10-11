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
