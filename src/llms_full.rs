//! PRD-mcphost-llms-full-generated-from-schemas: the file-level half of
//! `mcphost llms-full` -- loading the corpus and the prose exception list,
//! splicing the generated tool-sections block into `www/llms-full.txt`, and
//! the `--check` drift and prose-placement checks. The pure rendering lives
//! in [`crate::llms_txt`].

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::kinds::KindRegistry;
use crate::llms_txt::{TOOL_SECTIONS_END, TOOL_SECTIONS_START, anchor_slug, tenant_tool_names};
use crate::plans::PlanCatalog;

/// Hand-prose exception list, relative to the repo root.
pub const PROSE_PATH: &str = "docs/llms-full.prose.toml";

#[derive(Debug, Clone, Deserialize)]
pub struct ProseEntry {
    pub tool: String,
    /// `docs/…md#anchor`; no anchor means the whole file.
    pub path: String,
}

#[derive(Debug, Deserialize)]
struct ProseFile {
    #[serde(default)]
    section: Vec<ProseEntry>,
}

/// `Ok(None)` when the exception list does not exist.
pub fn load_prose(root: &Path) -> Result<Option<Vec<ProseEntry>>, String> {
    let path = root.join(PROSE_PATH);
    let Ok(text) = std::fs::read_to_string(&path) else { return Ok(None) };
    toml::from_str::<ProseFile>(&text)
        .map(|f| Some(f.section))
        .map_err(|e| format!("{}: {e}", path.display()))
}

fn heading_level(line: &str) -> Option<(usize, &str)> {
    let hashes = line.chars().take_while(|c| *c == '#').count();
    (hashes > 0 && line[hashes..].starts_with(' ')).then(|| (hashes, line[hashes..].trim()))
}

/// 1-based inclusive `(first, last)` line range of the section whose heading
/// slugs to `anchor`; the whole file when `anchor` is `None`.
fn section_range(text: &str, anchor: Option<&str>) -> Option<(usize, usize)> {
    let lines: Vec<&str> = text.lines().collect();
    let Some(anchor) = anchor else { return Some((1, lines.len().max(1))) };
    let (start, level) = lines.iter().enumerate().find_map(|(i, l)| {
        heading_level(l).filter(|(_, h)| anchor_slug(h) == anchor).map(|(lv, _)| (i, lv))
    })?;
    let end = lines[start + 1..]
        .iter()
        .position(|l| heading_level(l).is_some_and(|(lv, _)| lv <= level))
        .map_or(lines.len(), |p| start + 1 + p);
    Some((start + 1, end))
}

fn split_path(path: &str) -> (&str, Option<&str>) {
    match path.split_once('#') {
        Some((p, a)) => (p, Some(a)),
        None => (path, None),
    }
}

/// R3: each entry's markdown section, keyed by tool.
fn load_prose_text(root: &Path, entries: &[ProseEntry]) -> Result<BTreeMap<String, String>, String> {
    let mut out: BTreeMap<String, String> = BTreeMap::new();
    for e in entries {
        let (file, anchor) = split_path(&e.path);
        let text = std::fs::read_to_string(root.join(file)).map_err(|err| format!("{PROSE_PATH}: read {file}: {err}"))?;
        let (first, last) =
            section_range(&text, anchor).ok_or_else(|| format!("{PROSE_PATH}: no heading `#{}` in {file}", anchor.unwrap_or("")))?;
        let body = text.lines().skip(first - 1).take(last - first + 1).collect::<Vec<_>>().join("\n");
        let slot = out.entry(e.tool.clone()).or_default();
        if !slot.is_empty() {
            slot.push_str("\n\n");
        }
        slot.push_str(&body);
    }
    Ok(out)
}

fn is_name_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// The tools of `names` mentioned in `line`, matched on name boundaries
/// (`host.tool_share` is not a mention of `host.tool_share_x`).
fn mentions<'a>(line: &str, names: &'a [String]) -> Vec<&'a str> {
    let mut found = Vec::new();
    for name in names {
        let mut from = 0;
        while let Some(i) = line[from..].find(name.as_str()) {
            let at = from + i;
            let before = line[..at].chars().next_back();
            let after = &line[at + name.len()..];
            let mut a = after.chars();
            let next = a.next();
            let boundary_after = match next {
                None => true,
                Some(c) if is_name_char(c) => false,
                Some('.') => !a.next().is_some_and(is_name_char),
                Some(_) => true,
            };
            if boundary_after && !before.is_some_and(|c| is_name_char(c) || c == '.') {
                found.push(name.as_str());
                break;
            }
            from = at + name.len();
        }
    }
    found
}

fn scan_files(root: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    if root.join("README.md").is_file() {
        files.push(PathBuf::from("README.md"));
    }
    let mut pages = crate::docs_examples::corpus_pages(root).unwrap_or_default();
    pages.retain(|p| p != crate::gendocs::TOOLS_DOC_PATH);
    files.extend(pages.into_iter().map(PathBuf::from));
    files
}

/// R3: `file:line: …` for every source line that mentions a tool outside
/// the section the exception list gives that tool.
pub fn check_prose(root: &Path, entries: &[ProseEntry], names: &[String]) -> Vec<String> {
    let mut findings = Vec::new();
    for rel in scan_files(root) {
        let Ok(text) = std::fs::read_to_string(root.join(&rel)) else { continue };
        let rel_str = rel.to_string_lossy().replace('\\', "/");
        for (i, line) in text.lines().enumerate() {
            let lineno = i + 1;
            for tool in mentions(line, names) {
                let listed = entries.iter().any(|e| {
                    let (file, anchor) = split_path(&e.path);
                    e.tool == tool
                        && file == rel_str
                        && section_range(&text, anchor).is_some_and(|(a, b)| (a..=b).contains(&lineno))
                });
                if !listed {
                    findings.push(format!(
                        "{rel_str}:{lineno}: `{tool}` is mentioned outside its generated section and not listed in {PROSE_PATH}"
                    ));
                }
            }
        }
    }
    findings
}

/// Replace the marked block in `existing`, or insert it before the first
/// `## Kind:` section (else append).
pub fn splice_block(existing: &str, block: &str) -> String {
    let block = block.trim_end();
    if let (Some(start), Some(end)) = (existing.find(TOOL_SECTIONS_START), existing.find(TOOL_SECTIONS_END)) {
        let end = end + TOOL_SECTIONS_END.len();
        return format!("{}{}{}", &existing[..start], block, &existing[end..]);
    }
    if let Some(idx) = existing.find("\n## Kind:") {
        return format!("{}\n{}\n{}", &existing[..idx], block, &existing[idx..]);
    }
    format!("{}\n\n{}\n", existing.trim_end(), block)
}

fn block_of(text: &str) -> Option<&str> {
    let start = text.find(TOOL_SECTIONS_START)?;
    let end = text.find(TOOL_SECTIONS_END)? + TOOL_SECTIONS_END.len();
    Some(&text[start..end])
}

/// The first `### …` section (by heading) where `have` differs from `want`.
fn first_stale_section(have: &str, want: &str) -> String {
    let split = |s: &str| -> Vec<(String, String)> {
        s.split("\n### ")
            .map(|chunk| (chunk.lines().next().unwrap_or("").to_string(), chunk.to_string()))
            .collect()
    };
    let (h, w) = (split(have), split(want));
    for (i, (name, body)) in w.iter().enumerate() {
        match h.get(i) {
            Some((_, hb)) if hb == body => {}
            _ => return format!("`{}`", name.trim_start_matches("### ")),
        }
    }
    "(trailing sections)".to_string()
}

/// `mcphost llms-full [--check]`; returns the process exit code.
pub fn run(root: &Path, file: &Path, check: bool) -> i32 {
    let corpus = match std::fs::read_to_string(root.join(crate::docs_examples::CORPUS_PATH)) {
        Ok(_) => match crate::docs_examples::load_corpus(&root.join(crate::docs_examples::CORPUS_PATH)) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("llms-full: {e}");
                return 2;
            }
        },
        Err(_) => Vec::new(),
    };
    let entries = match load_prose(root) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("llms-full: {e}");
            return 2;
        }
    };
    let prose = match entries.as_deref().map(|e| load_prose_text(root, e)).transpose() {
        Ok(p) => p.unwrap_or_default(),
        Err(e) => {
            eprintln!("llms-full: {e}");
            return 2;
        }
    };
    let kinds = KindRegistry::with_builtin();
    let block = crate::llms_txt::render_tool_sections_with_prose(&kinds, &PlanCatalog::default_catalog(), &corpus, &prose);
    let existing = std::fs::read_to_string(file).unwrap_or_default();
    let updated = splice_block(&existing, &block);

    if !check {
        if let Err(e) = std::fs::write(file, &updated) {
            eprintln!("llms-full: write {}: {e}", file.display());
            return 2;
        }
        println!("llms-full: wrote {}", file.display());
        return 0;
    }

    let mut failed = false;
    if updated != existing {
        let which = match block_of(&existing) {
            Some(have) => first_stale_section(have, &block),
            None => "(no tool-sections block)".to_string(),
        };
        eprintln!(
            "llms-full --check: {}: section {which} is stale (run `mcphost llms-full`)",
            file.display()
        );
        failed = true;
    }
    match &entries {
        Some(entries) => {
            for f in check_prose(root, entries, &tenant_tool_names(&kinds)) {
                eprintln!("llms-full --check: {f}");
                failed = true;
            }
        }
        None => println!("llms-full --check: prose exception check not applicable (no {PROSE_PATH})"),
    }
    i32::from(failed)
}
