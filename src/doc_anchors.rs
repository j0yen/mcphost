//! PRD-mcphost-emitted-doc-pointers-resolve: every `<path>#<slug>` pointer
//! the host prints is a [`DocAnchor`], a variant `build.rs` generates from
//! the headings of README.md, `docs/**/*.md`, `www/llms.txt` and
//! `www/llms-install.md`. A pointer typed as a string literal cannot be
//! checked; a variant naming a heading that does not exist does not
//! compile.
//!
//! This file is the only place in `src/` allowed to spell a `.md#` anchor
//! (`tests/edp_ac02_*` scans for any other).

use serde_json::{Value, json};

use crate::errors::AppError;

pub use crate::doc_scan::{SECTION_MAX_BYTES, slugify};

/// One heading's generated row.
#[derive(Debug)]
pub struct AnchorEntry {
    /// `<path>#<slug>`.
    pub anchor: &'static str,
    pub path: &'static str,
    pub title: &'static str,
    pub level: u8,
    /// 1-based line of the heading in `path`.
    pub line: u32,
    /// The heading's body, sub-headings included, at most
    /// [`SECTION_MAX_BYTES`].
    pub text: &'static str,
    /// `text` was cut at a paragraph boundary to fit.
    pub truncated: bool,
    /// The first fenced block of `text`: `(lang, code)`.
    pub example: Option<(&'static str, &'static str)>,
}

include!(concat!(env!("OUT_DIR"), "/doc_anchors.rs"));

impl DocAnchor {
    /// Every anchor, in `ENTRIES` order.
    pub const ALL: &'static [DocAnchor] = ALL;

    fn entry(self) -> &'static AnchorEntry {
        &ENTRIES[self as usize]
    }

    /// `<path>#<slug>`.
    pub fn as_str(self) -> &'static str {
        self.entry().anchor
    }

    pub fn path(self) -> &'static str {
        self.entry().path
    }

    pub fn title(self) -> &'static str {
        self.entry().title
    }

    pub fn line(self) -> u32 {
        self.entry().line
    }

    /// The heading's body up to the next heading of the same or higher
    /// level (sub-headings included), at most 4 KB.
    pub fn section_text(self) -> &'static str {
        self.entry().text
    }

    /// The section was longer than 4 KB and [`Self::section_text`] stops at
    /// a paragraph boundary.
    pub fn truncated(self) -> bool {
        self.entry().truncated
    }

    /// The first fenced block of [`Self::section_text`] as `(lang, code)`.
    pub fn first_example(self) -> Option<(&'static str, &'static str)> {
        self.entry().example
    }

    /// The anchor spelled `<path>#<slug>`, if a heading has it.
    pub fn parse(anchor: &str) -> Option<DocAnchor> {
        ENTRIES.iter().position(|e| e.anchor == anchor).map(|i| ALL[i])
    }

    /// Up to `n` anchors nearest `anchor` by edit distance, nearest first.
    pub fn nearest(anchor: &str, n: usize) -> Vec<DocAnchor> {
        let mut scored: Vec<(usize, DocAnchor)> =
            ALL.iter().map(|a| (edit_distance(anchor, a.as_str()), *a)).collect();
        scored.sort_by_key(|(d, _)| *d);
        scored.into_iter().take(n).map(|(_, a)| a).collect()
    }
}

/// Levenshtein distance over chars.
fn edit_distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut cur = vec![i + 1];
        for (j, cb) in b.iter().enumerate() {
            let sub = prev[j] + usize::from(ca != *cb);
            cur.push(sub.min(prev[j + 1] + 1).min(cur[j] + 1));
        }
        prev = cur;
    }
    prev[b.len()]
}

/// Nearest anchors `anchor_not_found` offers.
const DID_YOU_MEAN: usize = 3;

pub const HELP_GET_DESCRIPTION: &str = "Return the documentation section an anchor points at: \
    its text (at most 4 KB, cut at a paragraph boundary with truncated: true when longer) and its first fenced example. Pass an anchor exactly as an error, \
    hint or help entry printed it, e.g. a rate_limited error's `anchor`. Read-only, anonymous, \
    no tenant data. An unknown anchor is anchor_not_found with the nearest anchors in did_you_mean.";

/// The `host.help.get` input schema properties (`host_schema` adds
/// `tenant_key`).
pub fn help_get_schema_properties() -> Value {
    json!({
        "anchor": {
            "type": "string",
            "description": "A `<path>#<slug>` anchor, e.g. README.md#call-limits, as printed by an error's `anchor` field or a `host.help.search` hit.",
        },
    })
}

/// `host.help.get`: `args` is the call's argument object.
pub fn help_get(args: &Value) -> Result<Value, AppError> {
    let anchor = match args.get("anchor") {
        Some(Value::String(a)) if !a.trim().is_empty() => a.trim(),
        _ => return Err(AppError::InvalidArgs("anchor is required: a `<path>#<slug>` string such as README.md#call-limits".to_string())),
    };
    let Some(found) = DocAnchor::parse(anchor) else {
        let near: Vec<&str> = DocAnchor::nearest(anchor, DID_YOU_MEAN).into_iter().map(DocAnchor::as_str).collect();
        return Err(AppError::Structured {
            code: "anchor_not_found",
            message: format!("no documentation heading has the anchor '{anchor}'; did you mean {}?", near.join(", ")),
            data: json!({"did_you_mean": near, "hint": "host.help.search"}),
        });
    };
    let mut body = json!({
        "anchor": found.as_str(),
        "title": found.title(),
        "text": found.section_text(),
        "truncated": found.truncated(),
        "path": found.path(),
        "line": found.line(),
    });
    if let Some((lang, code)) = found.first_example() {
        body["example"] = json!({"lang": lang, "code": code});
    }
    Ok(body)
}

/// One pointer found in `src/`: where it is emitted and what it names.
#[derive(Debug, Clone, PartialEq)]
pub struct EmittedPointer {
    /// Repo-relative path of the emitting file.
    pub file: String,
    /// 1-based line.
    pub line: usize,
    pub anchor: String,
}

/// PRD-mcphost-emitted-doc-pointers-resolve-b R5: the result of checking a
/// repo tree's emitted pointers against its headings on disk.
#[derive(Debug)]
pub struct CheckReport {
    /// Headings found on disk.
    pub anchors: usize,
    pub emitted: Vec<EmittedPointer>,
    pub unresolved: Vec<EmittedPointer>,
}

fn rust_files(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for entry in rd.filter_map(|e| e.ok()) {
        let path = entry.path();
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// `.md#slug` literals on `line`: the whole `<path>.md#<slug>` token.
fn md_literals(line: &str) -> Vec<String> {
    let mut found = Vec::new();
    let mut from = 0;
    while let Some(i) = line[from..].find(".md#") {
        let at = from + i;
        let start = line[..at]
            .rfind(|c: char| !(c.is_ascii_alphanumeric() || "/._-".contains(c)))
            .map_or(0, |p| p + 1);
        let end = line[at + 4..]
            .find(|c: char| !(c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'))
            .map_or(line.len(), |p| at + 4 + p);
        // A bare `README.md#` prefix (a `starts_with` test) names no heading.
        if end > at + 4 {
            found.push(line[start..end].to_string());
        }
        from = at + 4;
    }
    found
}

/// Every pointer the Rust sources under `root/src` emit: a `DocAnchor::Variant`
/// use (resolved to its compiled anchor string) or a `.md#` literal. Comment
/// lines and `src/doc_anchors.rs` itself are skipped.
pub fn emitted_pointers(root: &std::path::Path) -> Vec<EmittedPointer> {
    let by_name: std::collections::HashMap<String, &'static str> =
        DocAnchor::ALL.iter().map(|a| (format!("{a:?}"), a.as_str())).collect();
    let mut files = Vec::new();
    rust_files(&root.join("src"), &mut files);
    files.sort();
    let mut out = Vec::new();
    for path in files {
        let Ok(rel) = path.strip_prefix(root) else { continue };
        let rel = rel.to_string_lossy().replace('\\', "/");
        if rel == "src/doc_anchors.rs" {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else { continue };
        for (i, line) in text.lines().enumerate() {
            if line.trim_start().starts_with("//") {
                continue;
            }
            let mut rest = line;
            while let Some(p) = rest.find("DocAnchor::") {
                rest = &rest[p + "DocAnchor::".len()..];
                let name: String = rest.chars().take_while(|c| c.is_ascii_alphanumeric()).collect();
                if let Some(anchor) = by_name.get(&name) {
                    out.push(EmittedPointer { file: rel.clone(), line: i + 1, anchor: (*anchor).to_string() });
                }
            }
            for anchor in md_literals(line) {
                out.push(EmittedPointer { file: rel.clone(), line: i + 1, anchor });
            }
        }
    }
    out
}

/// Check every pointer `root/src` emits against the headings of the docs
/// under `root` as they are on disk now (not the set compiled into this
/// binary), so a renamed heading in a copy of the repo is found.
pub fn check_repo(root: &std::path::Path) -> CheckReport {
    let mut live = std::collections::HashSet::new();
    for rel in crate::doc_scan::doc_paths(root) {
        if let Ok(text) = std::fs::read_to_string(root.join(&rel)) {
            for h in crate::doc_scan::scan(&text) {
                live.insert(format!("{rel}#{}", h.slug));
            }
        }
    }
    let emitted = emitted_pointers(root);
    let unresolved = emitted.iter().filter(|p| !live.contains(&p.anchor)).cloned().collect();
    CheckReport { anchors: live.len(), emitted, unresolved }
}

/// The anchors `control.rs` prints beside its `docs` fields and hints (the
/// help entries' own `doc_link`s are read from [`crate::help::HELP_ENTRIES`]).
/// `tests/edpb_ac02_*` asserts every pointer `src/` emits is listed, so this
/// cannot fall behind the code.
pub const CONTROL_ANCHORS: &[DocAnchor] =
    &[DocAnchor::DocsToolsHostQuickstart, DocAnchor::DocsToolsHostTriggerSet, DocAnchor::DocsToolsHostChangelog];

/// Every distinct anchor the host emits, sorted.
pub fn emitted_anchors() -> Vec<&'static str> {
    let mut out: Vec<&'static str> = crate::help::HELP_ENTRIES
        .iter()
        .map(|e| e.doc_link)
        .chain(CONTROL_ANCHORS.iter().copied())
        .map(DocAnchor::as_str)
        .collect();
    out.sort_unstable();
    out.dedup();
    out
}
