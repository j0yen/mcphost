//! PRD-mcphost-help-search-tool: `host.help.search {q, kind?, k?}` -- a
//! lexical (FTS5, no model call) search over the host's own documentation
//! and tool descriptions, returning per hit a snippet, the nearest runnable
//! fenced example and an anchor, so an agent calls the docs instead of
//! fetching them.
//!
//! The corpus is README.md, `docs/**/*.md` and `www/llms.txt`, embedded at
//! compile time through the `build.rs` manifest ([`HELP_DOCS`]), plus every
//! `Tool` description from `handler::host_tools`. Documents are cut into
//! markdown-section chunks, each carrying `<path>#<heading-slug>` (or
//! `tool:<name>`), loaded into an in-memory FTS5 table once per process.
use std::sync::{Mutex, OnceLock};

use serde_json::{Value, json};

use crate::docs_index::sanitize_fts_query;
use crate::errors::AppError;
use crate::kinds::KindRegistry;

include!(concat!(env!("OUT_DIR"), "/help_docs_embedded.rs"));

/// Longest chunk body, split at paragraph boundaries.
const CHUNK_MAX_CHARS: usize = 1200;
/// `snippet` ceiling.
const SNIPPET_MAX_CHARS: usize = 400;
const DEFAULT_K: usize = 3;
const MAX_K: usize = 5;
/// How many FTS rows are pulled before filtering and de-duplicating.
const CANDIDATES: i64 = 200;
/// Words every question carries; they match nearly every chunk and only
/// dilute the ranking, so they are dropped from the query unless nothing
/// else is left.
const STOPWORDS: &[&str] = &[
    "a", "an", "and", "are", "as", "at", "be", "by", "for", "from", "how", "i", "in", "is", "it", "my", "no", "not",
    "of", "on", "or", "that", "the", "this", "to", "was", "what", "with",
];
const HINT: &str = "try a tool name, an error code or an import line";

/// One fenced block lifted out of a page.
#[derive(Debug, Clone, PartialEq)]
pub struct Example {
    pub lang: String,
    pub code: String,
    pub anchor: String,
    /// `true` once [`fit`] cut `code` to keep the response under budget.
    pub truncated: bool,
}

#[derive(Debug, Clone)]
pub struct Chunk {
    pub anchor: String,
    pub title: String,
    /// `Some(kind)` for a chunk of `docs/kinds/<kind>.md`.
    pub kind: Option<String>,
    pub text: String,
    pub example: Option<Example>,
    /// The tool a `tool:<name>` chunk describes.
    pub next_tool: Option<String>,
}

/// `lowercase, non-alphanumerics to '-'` (runs collapsed, edges trimmed):
/// the one slugger every anchor in this index uses.
pub fn slugify(heading: &str) -> String {
    let mut out = String::new();
    for c in heading.chars() {
        if c.is_alphanumeric() {
            out.extend(c.to_lowercase());
        } else if !out.is_empty() && !out.ends_with('-') {
            out.push('-');
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    out
}

struct Section {
    level: usize,
    title: String,
    slug: String,
    blocks: Vec<Block>,
}

enum Block {
    Prose(String),
    Fence { lang: String, code: String },
}

fn fence_lang(line: &str) -> Option<String> {
    let t = line.trim_start();
    t.strip_prefix("```").map(|rest| rest.split_whitespace().next().unwrap_or("").to_string())
}

fn parse_sections(path: &str, text: &str) -> Vec<Section> {
    let stem = path.rsplit('/').next().unwrap_or(path).trim_end_matches(".md").trim_end_matches(".txt");
    let mut sections = vec![Section { level: 0, title: stem.to_string(), slug: "top".into(), blocks: vec![] }];
    let mut prose = String::new();
    let mut fence: Option<(String, String)> = None;
    let mut seen: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    let flush = |sections: &mut Vec<Section>, prose: &mut String| {
        let p = prose.trim_matches('\n');
        if !p.trim().is_empty() {
            sections.last_mut().expect("section").blocks.push(Block::Prose(p.to_string()));
        }
        prose.clear();
    };
    for line in text.lines() {
        if let Some((lang, code)) = fence.as_mut() {
            if fence_lang(line).is_some() && line.trim() == "```" {
                let (lang, code) = (std::mem::take(lang), std::mem::take(code));
                fence = None;
                sections.last_mut().expect("section").blocks.push(Block::Fence { lang, code });
            } else {
                code.push_str(line);
                code.push('\n');
            }
            continue;
        }
        if let Some(lang) = fence_lang(line) {
            flush(&mut sections, &mut prose);
            fence = Some((lang, String::new()));
            continue;
        }
        let hashes = line.chars().take_while(|c| *c == '#').count();
        if (1..=6).contains(&hashes) && line[hashes..].starts_with(' ') {
            flush(&mut sections, &mut prose);
            let title = line[hashes..].trim().trim_matches('#').trim().to_string();
            let mut slug = slugify(&title);
            if slug.is_empty() {
                slug = "section".into();
            }
            let n = seen.entry(slug.clone()).or_insert(0);
            *n += 1;
            if *n > 1 {
                slug = format!("{slug}-{n}");
            }
            sections.push(Section { level: hashes, title, slug, blocks: vec![] });
            continue;
        }
        if line.trim().is_empty() && prose.ends_with("\n\n") {
            continue;
        }
        prose.push_str(line);
        prose.push('\n');
    }
    if let Some((lang, code)) = fence {
        sections.last_mut().expect("section").blocks.push(Block::Fence { lang, code });
    }
    flush(&mut sections, &mut prose);
    sections
}

/// A fenced block as an example: a ```json block whose object carries a
/// string `source` (the `host.tool_publish` spec shape) is decoded to the
/// runnable code itself, so the example is the program, not its JSON escape.
fn example_of(lang: &str, code: &str, anchor: &str) -> Example {
    if lang == "json"
        && let Ok(Value::Object(o)) = serde_json::from_str::<Value>(code)
        && let Some(Value::String(src)) = o.get("source")
    {
        return Example { lang: "python".into(), code: src.clone(), anchor: anchor.to_string(), truncated: false };
    }
    Example { lang: lang.to_string(), code: code.trim_end().to_string(), anchor: anchor.to_string(), truncated: false }
}

/// Cuts `s` into pieces of at most `max` chars, at line boundaries where
/// possible (a single over-long line is cut at a char boundary).
fn split_long(s: &str, max: usize) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for line in s.split_inclusive('\n') {
        let mut rest = line;
        while rest.chars().count() > max {
            if !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
            let cut = rest.char_indices().nth(max).map_or(rest.len(), |(i, _)| i);
            let cut = rest[..cut].rfind(' ').filter(|i| *i > max / 2).map_or(cut, |i| i + 1);
            out.push(rest[..cut].to_string());
            rest = &rest[cut..];
        }
        if cur.chars().count() + rest.chars().count() > max && !cur.is_empty() {
            out.push(std::mem::take(&mut cur));
        }
        cur.push_str(rest);
    }
    if !cur.trim().is_empty() {
        out.push(cur);
    }
    out
}

/// Markdown `text` of `path` as chunks: one per heading section, split at
/// paragraph boundaries into pieces of at most [`CHUNK_MAX_CHARS`] (a
/// fenced block is never split).
pub fn chunk_markdown(path: &str, text: &str) -> Vec<Chunk> {
    let kind = path
        .strip_prefix("docs/kinds/")
        .and_then(|f| f.strip_suffix(".md"))
        .map(str::to_string);
    let sections = parse_sections(path, text);
    let mut out = Vec::new();
    for (i, sec) in sections.iter().enumerate() {
        let anchor = format!("{path}#{}", sec.slug);
        // Nearest example: inside this section (sub-sections included), else
        // the next fenced block further down the same page.
        let fence_in = |s: &Section| {
            s.blocks.iter().find_map(|b| match b {
                Block::Fence { lang, code } => Some(example_of(lang, code, &format!("{path}#{}", s.slug))),
                Block::Prose(_) => None,
            })
        };
        let section_example = std::iter::once(sec)
            .chain(sections[i + 1..].iter().take_while(|s| s.level > sec.level))
            .find_map(fence_in)
            .or_else(|| sections[i + 1..].iter().find_map(fence_in));
        let mut pieces: Vec<(String, Option<Example>)> = Vec::new();
        let mut cur = String::new();
        let mut cur_ex: Option<Example> = None;
        for b in &sec.blocks {
            let (parts, ex): (Vec<String>, Option<Example>) = match b {
                Block::Prose(p) => (
                    p.split("\n\n")
                        .filter(|para| !para.trim().is_empty())
                        .flat_map(|para| {
                            if para.chars().count() <= CHUNK_MAX_CHARS {
                                vec![para.to_string()]
                            } else {
                                split_long(para, CHUNK_MAX_CHARS)
                            }
                        })
                        .collect(),
                    None,
                ),
                Block::Fence { lang, code } => {
                    (vec![format!("```{lang}\n{code}```")], Some(example_of(lang, code, &anchor)))
                }
            };
            for part in parts {
                if !cur.is_empty() && cur.chars().count() + part.chars().count() + 2 > CHUNK_MAX_CHARS {
                    pieces.push((std::mem::take(&mut cur), cur_ex.take()));
                }
                if !cur.is_empty() {
                    cur.push_str("\n\n");
                }
                cur.push_str(&part);
                if cur_ex.is_none() {
                    cur_ex = ex.clone();
                }
            }
        }
        if !cur.trim().is_empty() {
            pieces.push((cur, cur_ex));
        }
        for (body, ex) in pieces {
            out.push(Chunk {
                anchor: anchor.clone(),
                title: sec.title.clone(),
                kind: kind.clone(),
                text: body,
                example: ex.or_else(|| section_example.clone()),
                next_tool: None,
            });
        }
    }
    out
}

/// A chunk per tool: its description plus the names of its arguments.
fn tool_chunks(kinds: &KindRegistry) -> Vec<Chunk> {
    crate::handler::host_tools(kinds, true)
        .into_iter()
        .filter(|t| t.name.contains('.'))
        .map(|t| {
            let args: Vec<String> = t
                .input_schema
                .get("properties")
                .and_then(Value::as_object)
                .map(|p| p.keys().filter(|k| *k != "tenant_key").cloned().collect())
                .unwrap_or_default();
            let description = t.description.as_deref().unwrap_or_default();
            let text = if args.is_empty() {
                description.to_string()
            } else {
                format!("{description}\n\nArguments: {}", args.join(", "))
            };
            Chunk {
                anchor: format!("tool:{}", t.name),
                title: t.name.to_string(),
                kind: None,
                text,
                example: None,
                next_tool: Some(t.name.to_string()),
            }
        })
        .collect()
}

/// Every chunk of the embedded docs, in manifest order.
pub fn doc_chunks() -> Vec<Chunk> {
    HELP_DOCS.iter().flat_map(|(path, text)| chunk_markdown(path, text)).collect()
}

pub struct HelpIndex {
    chunks: Vec<Chunk>,
    conn: Mutex<rusqlite::Connection>,
    version: String,
}

impl HelpIndex {
    pub fn build(kinds: &KindRegistry) -> Self {
        let mut chunks = doc_chunks();
        chunks.extend(tool_chunks(kinds));
        let conn = rusqlite::Connection::open_in_memory().expect("in-memory sqlite");
        conn.execute_batch("CREATE VIRTUAL TABLE h USING fts5(title, body, tokenize = 'porter unicode61')")
            .expect("create help fts table");
        {
            let mut ins = conn.prepare("INSERT INTO h(rowid, title, body) VALUES (?1, ?2, ?3)").expect("prepare");
            for (i, c) in chunks.iter().enumerate() {
                ins.execute(rusqlite::params![i as i64, c.title, c.text]).expect("insert chunk");
            }
        }
        use sha2::{Digest, Sha256};
        let mut h = Sha256::new();
        for c in &chunks {
            h.update(c.anchor.as_bytes());
            h.update(c.text.as_bytes());
        }
        let version: String = h.finalize().iter().take(6).map(|b| format!("{b:02x}")).collect();
        Self { chunks, conn: Mutex::new(conn), version }
    }

    pub fn chunk_count(&self) -> usize {
        self.chunks.len()
    }

    pub fn version(&self) -> &str {
        &self.version
    }

    /// Ranked, de-duplicated (one per anchor) chunk indexes with their
    /// scores (higher is better), best first.
    fn ranked(&self, q: &str) -> Vec<(usize, f64)> {
        let Some(m) = match_expr(q) else { return Vec::new() };
        let conn = self.conn.lock().expect("help index lock");
        let Ok(mut stmt) = conn
            .prepare("SELECT rowid, bm25(h, 3.0, 1.0) FROM h WHERE h MATCH ?1 ORDER BY bm25(h, 3.0, 1.0) LIMIT ?2")
        else {
            return Vec::new();
        };
        let rows: Vec<(usize, f64)> = stmt
            .query_map(rusqlite::params![m, CANDIDATES], |r| Ok((r.get::<_, i64>(0)? as usize, -r.get::<_, f64>(1)?)))
            .map(|it| it.filter_map(Result::ok).collect())
            .unwrap_or_default();
        let mut rows: Vec<(usize, f64)> =
            rows.into_iter().map(|(i, s)| (i, s * authority(&self.chunks[i].anchor))).collect();
        rows.sort_by(|a, b| b.1.total_cmp(&a.1));
        let mut seen = std::collections::HashSet::new();
        rows.into_iter().filter(|(i, _)| seen.insert(self.chunks[*i].anchor.clone())).collect()
    }

    /// The hits for `q`; `total` is the number of distinct matching anchors.
    ///
    /// `kind` keeps only chunks of `docs/kinds/<kind>.md`.
    pub fn search(&self, q: &str, k: usize, kind: Option<&str>) -> (Vec<Hit>, usize) {
        let mut ranked = self.ranked(q);
        if let Some(kind) = kind {
            ranked.retain(|(i, _)| self.chunks[*i].kind.as_deref() == Some(kind));
        }
        let total = ranked.len();
        let terms = content_terms(q);
        let hits = ranked
            .into_iter()
            .take(k)
            .map(|(i, score)| {
                let c = &self.chunks[i];
                Hit {
                    anchor: c.anchor.clone(),
                    title: c.title.clone(),
                    snippet: snippet(&c.text, &terms, SNIPPET_MAX_CHARS),
                    example: c.example.clone(),
                    score,
                    next_tool: c.next_tool.clone(),
                    text: c.text.clone(),
                }
            })
            .collect();
        (hits, total)
    }
}

#[derive(Debug, Clone)]
pub struct Hit {
    pub anchor: String,
    pub title: String,
    pub snippet: String,
    pub example: Option<Example>,
    pub score: f64,
    pub next_tool: Option<String>,
    /// The chunk text the snippet was cut from, kept so [`fit`] can re-cut it.
    text: String,
}

fn query_terms(q: &str) -> Vec<String> {
    q.split(|c: char| !c.is_alphanumeric()).filter(|s| !s.is_empty()).map(str::to_ascii_lowercase).collect()
}

/// [`query_terms`] without [`STOPWORDS`] (kept whole if every term is one).
fn content_terms(q: &str) -> Vec<String> {
    let all = query_terms(q);
    let kept: Vec<String> = all.iter().filter(|t| !STOPWORDS.contains(&t.as_str())).cloned().collect();
    if kept.is_empty() { all } else { kept }
}

/// The FTS5 match for `q`: [`sanitize_fts_query`] over the content terms
/// (every term, OR'd) plus quoted two- and three-word phrases of
/// consecutive terms and each whitespace-separated identifier as one phrase
/// (`tenant_key_missing` -> "tenant key missing"), so a chunk that carries
/// the error text itself outranks one that merely shares a common word.
fn match_expr(q: &str) -> Option<String> {
    let terms = content_terms(q);
    let mut expr = sanitize_fts_query(&terms.join(" "))?;
    let mut phrases: Vec<String> = Vec::new();
    for word in q.split_whitespace() {
        let parts = query_terms(word);
        if parts.len() >= 2 {
            phrases.push(parts.join(" "));
        }
    }
    for n in [2usize, 3] {
        phrases.extend(terms.windows(n).map(|w| w.join(" ")));
    }
    phrases.sort();
    phrases.dedup();
    for p in phrases.iter().take(40) {
        expr.push_str(&format!(" OR \"{p}\""));
    }
    Some(expr)
}

/// How much a chunk's page is trusted as the answer: kind pages are the
/// reference for their kind; receipts, benchmark dumps and launch notes are
/// history that merely quotes the reference.
fn authority(anchor: &str) -> f64 {
    if anchor.starts_with("docs/kinds/") && anchor.ends_with("#kind-ceilings") {
        // A kind page's ceilings table only restates `docs/plans.md`'s numbers.
        0.5
    } else if anchor.starts_with("docs/kinds/") {
        1.5
    } else if anchor.starts_with("docs/receipts/")
        || anchor.starts_with("docs/benchmarks/")
        || anchor.starts_with("docs/launch/")
    {
        0.4
    } else {
        1.0
    }
}

/// At most `max` chars of `text` (fences and runs of whitespace collapsed),
/// windowed to start just before the first matched term.
fn snippet(text: &str, terms: &[String], max: usize) -> String {
    let flat: String = text
        .lines()
        .filter(|l| !l.trim_start().starts_with("```"))
        .collect::<Vec<_>>()
        .join(" ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let lower = flat.to_ascii_lowercase();
    let pos = terms.iter().filter_map(|t| lower.find(t.as_str())).min().unwrap_or(0);
    let mut start = pos.saturating_sub(40);
    while !flat.is_char_boundary(start) {
        start -= 1;
    }
    if start > 0
        && let Some(sp) = flat[start..].find(' ')
    {
        start += sp + 1;
    }
    flat[start..].chars().take(max).collect()
}

fn render(hits: &[Hit], total: usize, version: &str) -> Value {
    let hits: Vec<Value> = hits
        .iter()
        .map(|h| {
            let mut o = json!({
                "anchor": h.anchor,
                "title": h.title,
                "snippet": h.snippet,
                "score": (h.score * 100.0).round() / 100.0,
            });
            if let Some(e) = &h.example {
                o["example"] = json!({"lang": e.lang, "code": e.code, "anchor": e.anchor});
                if e.truncated {
                    o["example"]["truncated"] = json!(true);
                }
            }
            if let Some(t) = &h.next_tool {
                o["next_tool"] = json!(t);
            }
            o
        })
        .collect();
    json!({"hits": hits, "total": total, "index_version": version})
}

/// Hard ceiling on the serialized response.
pub const RESPONSE_MAX_BYTES: usize = 2048;

fn size_of(hits: &[Hit], total: usize, version: &str) -> usize {
    render(hits, total, version).to_string().len()
}

/// `s` cut to at most `max` bytes at a char boundary, back to the last line
/// break when that loses under half of it.
fn cut_code(s: &str, max: usize) -> String {
    let mut end = max.min(s.len());
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    let cut = &s[..end];
    match cut.rfind('\n') {
        Some(nl) if nl >= end / 2 => cut[..nl].to_string(),
        _ => cut.to_string(),
    }
}

/// Shrinks `hits` until [`render`] serializes to at most
/// [`RESPONSE_MAX_BYTES`]: snippets first, then the longest example is
/// cut (and marked `truncated`), then snippets go entirely, then the
/// weakest hits.
fn fit(hits: &mut Vec<Hit>, total: usize, version: &str, terms: &[String]) {
    let over = |hits: &[Hit]| size_of(hits, total, version).saturating_sub(RESPONSE_MAX_BYTES);
    if over(hits) > 0 {
        for h in hits.iter_mut() {
            h.snippet = snippet(&h.text, terms, 160);
        }
    }
    while over(hits) > 0 {
        let excess = over(hits);
        let Some(longest) = hits
            .iter_mut()
            .filter_map(|h| h.example.as_mut())
            .filter(|e| !e.code.is_empty())
            .max_by_key(|e| e.code.len())
        else {
            break;
        };
        let keep = longest.code.len().saturating_sub(excess);
        longest.code = cut_code(&longest.code, keep);
        longest.truncated = true;
    }
    for cap in [60, 0] {
        if over(hits) > 0 {
            for h in hits.iter_mut() {
                h.snippet = snippet(&h.text, terms, cap);
            }
        }
    }
    while over(hits) > 0 && hits.len() > 1 {
        hits.pop();
    }
}

static INDEX: OnceLock<HelpIndex> = OnceLock::new();

/// The process-wide index, built on first use from `kinds`.
pub fn index(kinds: &KindRegistry) -> &'static HelpIndex {
    INDEX.get_or_init(|| HelpIndex::build(kinds))
}

/// PRD-mcphost-help-search-tool R7: called once per request with the
/// normalized query and the top hit's anchor. The telemetry PRD fills this.
pub fn observe(_normalized_query: &str, _top_anchor: Option<&str>) {}

fn args_invalid(message: String) -> AppError {
    AppError::InvalidArgs(message)
}

/// `host.help.search`: `args` is the call's argument object.
pub fn call(kinds: &KindRegistry, args: &Value) -> Result<Value, AppError> {
    let k = match args.get("k") {
        None | Some(Value::Null) => DEFAULT_K,
        Some(v) => match v.as_u64() {
            Some(n) if (1..=MAX_K as u64).contains(&n) => n as usize,
            _ => return Err(args_invalid(format!("k must be an integer from 1 to {MAX_K}"))),
        },
    };
    let registered = kinds.names();
    let kind = match args.get("kind") {
        None | Some(Value::Null) => None,
        Some(Value::String(name)) if registered.contains(&name.as_str()) => Some(name.as_str()),
        Some(other) => {
            let given = other.as_str().map_or_else(|| other.to_string(), str::to_string);
            return Err(args_invalid(format!(
                "kind '{given}' is not a registered kind; registered kinds: {}",
                registered.join(", ")
            )));
        }
    };
    let q = args.get("q").and_then(Value::as_str).unwrap_or_default();
    let idx = index(kinds);
    let terms = query_terms(q);
    if terms.is_empty() {
        return Ok(json!({"hits": [], "total": 0, "hint": HINT, "index_version": idx.version()}));
    }
    let (mut hits, total) = idx.search(q, k, kind);
    fit(&mut hits, total, idx.version(), &content_terms(q));
    observe(&terms.join(" "), hits.first().map(|h| h.anchor.as_str()));
    Ok(render(&hits, total, idx.version()))
}

/// The CLI's readable form of a [`call`] result.
pub fn render_text(body: &Value) -> String {
    let hits = body["hits"].as_array().cloned().unwrap_or_default();
    if hits.is_empty() {
        return format!("no hits; {}\n", body["hint"].as_str().unwrap_or(HINT));
    }
    let mut out = String::new();
    for hit in &hits {
        out.push_str(&format!("{}\n  {}\n", hit["anchor"].as_str().unwrap_or_default(), hit["snippet"].as_str().unwrap_or_default()));
        if let Some(code) = hit["example"]["code"].as_str() {
            out.push_str(&format!("  example ({}):\n", hit["example"]["lang"].as_str().unwrap_or_default()));
            for line in code.lines() {
                out.push_str(&format!("    {line}\n"));
            }
        }
    }
    out
}

/// The `host.help.search` input schema properties (`host_schema` adds
/// `tenant_key`).
pub fn schema_properties(kinds: &KindRegistry) -> Value {
    json!({
        "q": {
            "type": "string",
            "description": "What you are stuck on: a tool name, an error code or message, an import line.",
        },
        "kind": {
            "type": "string",
            "enum": kinds.names(),
            "description": "Only search the page of this registered kind (docs/kinds/<kind>.md).",
        },
        "k": {
            "type": "integer", "minimum": 1, "maximum": MAX_K, "default": DEFAULT_K,
            "description": "How many hits to return (default 3).",
        },
    })
}

pub const TOOL_DESCRIPTION: &str = "Search this host's own documentation and tool descriptions. \
    Returns up to k hits (default 3, response under 2 KB), each with a snippet, the nearest \
    runnable example and an anchor. Read-only, anonymous, no tenant data -- call it with the \
    error text or import line you are stuck on before reading any docs file.";
