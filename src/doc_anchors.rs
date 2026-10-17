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

    /// The first fenced block of [`Self::section_text`] as `(lang, code)`.
    pub fn first_example(self) -> Option<(&'static str, &'static str)> {
        self.entry().example
    }

    /// The anchor spelled `<path>#<slug>`, if a heading has it. Also
    /// `<path>#top` and a bare `<path>` (no `#`): both resolve to the first
    /// generated anchor of that path, unless a real heading owns the slug.
    pub fn parse(anchor: &str) -> Option<DocAnchor> {
        if let Some(i) = ENTRIES.iter().position(|e| e.anchor == anchor) {
            return Some(ALL[i]);
        }
        let path = anchor.strip_suffix(TOP_SLUG).unwrap_or(anchor);
        if path.contains('#') {
            return None;
        }
        Self::first_of_path(path)
    }

    /// The first generated anchor whose document is `path`.
    pub fn first_of_path(path: &str) -> Option<DocAnchor> {
        ENTRIES.iter().position(|e| e.path == path).map(|i| ALL[i])
    }

    /// Up to `n` anchors of the document `anchor` names (the part before
    /// `#`, or all of it), nearest first; empty when no document has that
    /// path.
    pub fn nearest_in_path(anchor: &str, n: usize) -> Vec<DocAnchor> {
        let path = anchor.split_once('#').map_or(anchor, |(p, _)| p);
        let mut scored: Vec<(usize, DocAnchor)> = ALL
            .iter()
            .filter(|a| a.path() == path)
            .map(|a| (edit_distance(anchor, a.as_str()), *a))
            .collect();
        scored.sort_by_key(|(d, _)| *d);
        scored.into_iter().take(n).map(|(_, a)| a).collect()
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

/// The slug `help.get` accepts for "the document's first section".
const TOP_SLUG: &str = "#top";

/// Nearest anchors `anchor_not_found` offers.
const DID_YOU_MEAN: usize = 3;

pub const HELP_GET_DESCRIPTION: &str = "Return the documentation section an anchor points at: \
    its text (at most 4 KB) and its first fenced example. Pass an anchor exactly as an error, \
    hint or help entry printed it, e.g. a rate_limited error's `anchor`. Read-only, anonymous, \
    no tenant data. An unknown anchor is anchor_not_found with the nearest anchors in did_you_mean.";

/// The `host.help.get` input schema properties (`host_schema` adds
/// `tenant_key`).
pub fn help_get_schema_properties() -> Value {
    json!({
        "anchor": {
            "type": "string",
            "description": "A `<path>#<slug>` anchor, e.g. README.md#call-limits, as printed by an error's `anchor` field or a `host.help.search` hit. `<path>#top` and a bare `<path>` (e.g. docs/kinds/chain.md) both return that document's first section.",
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
        // A known document with an unknown slug: that document's own
        // anchors first, then the global nearest to fill the list.
        let mut near_anchors = DocAnchor::nearest_in_path(anchor, DID_YOU_MEAN);
        for extra in DocAnchor::nearest(anchor, DID_YOU_MEAN) {
            if near_anchors.len() < DID_YOU_MEAN && !near_anchors.contains(&extra) {
                near_anchors.push(extra);
            }
        }
        let near: Vec<&str> = near_anchors.into_iter().map(DocAnchor::as_str).collect();
        return Err(AppError::Structured {
            code: ANCHOR_NOT_FOUND,
            message: format!("no documentation heading has the anchor '{anchor}'; did you mean {}?", near.join(", ")),
            data: json!({"did_you_mean": near, "hint": "host.help.search"}),
        });
    };
    let mut body = json!({
        "anchor": found.as_str(),
        "title": found.title(),
        "text": found.section_text(),
        "path": found.path(),
        "line": found.line(),
    });
    if let Some((lang, code)) = found.first_example() {
        body["example"] = json!({"lang": lang, "code": code});
    }
    Ok(body)
}

crate::err_code! {
    pub ANCHOR_NOT_FOUND,
    code: "anchor_not_found", family: Args, class: AgentFault, retryable: false,
    remedy: "Pick one of data.did_you_mean, or call host.help.search for a real anchor.",
}

