//! PRD-mcphost-error-codes-generated-registry: the scanner behind the
//! `cargo test --lib` gate. Walks a tree of `*.rs` files and reports every
//! place an error code is written as a bare literal where an `err_code!`
//! const belongs, so a missing registry row fails inside the inner iteration
//! (naming file:line) instead of at CI.
//!
//! Textual, not syntactic -- the same shape as `errjson_ac01`'s scan, plus
//! the exemptions the declaration form needs (`code:` inside an `err_code!`
//! invocation is the declaration itself). Lines that are comments are skipped.

use std::fmt;
use std::path::{Path, PathBuf};

/// Prefixes after which a code literal is a raise site.
//
// Assembled with `concat!` so this file's own text never contains a prefix
// followed by a quote, which `errjson_ac01`'s textual backstop would read as
// a code literal.
const CODE_FIELD: &str = concat!("co", "de:");
const PREFIXES: [&str; 4] =
    [CODE_FIELD, concat!("structured", "("), concat!("structured_with", "("), "\"error_code\":"];

/// What a violation is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViolationKind {
    /// A string literal where an `err_code!` const belongs.
    BareLiteral,
    /// A code built with `format!` after a raise-site prefix: it cannot be
    /// enumerated into the registry.
    FormatBuilt,
    /// The same code declared by `err_code!` at two sites.
    Duplicate,
}

/// One scanner finding, naming `file:line`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Violation {
    pub kind: ViolationKind,
    /// Path relative to the scanned root's parent (so `src/foo.rs`).
    pub file: String,
    pub line: usize,
    /// The offending code.
    pub code: String,
    /// Which prefix (or `AppError::code() arm`) the literal followed.
    pub context: String,
    /// For [`ViolationKind::Duplicate`]: the first declaration's `file:line`.
    pub first_site: Option<(String, usize)>,
}

impl Violation {
    /// The `err_code!` template prefilled with the offending code.
    pub fn template(&self) -> String {
        format!(
            "err_code! {{ pub {}, code: \"{}\", family: ?, class: ?, retryable: false, remedy: \"?\" }}",
            self.code.to_ascii_uppercase(),
            self.code
        )
    }
}

impl fmt::Display for Violation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.kind {
            ViolationKind::FormatBuilt => write!(
                f,
                "{}:{}: code built with format! after `{}`; a code must be a static err_code! const",
                self.file, self.line, self.context
            ),
            ViolationKind::Duplicate => {
                let (first_file, first_line) = self.first_site.clone().unwrap_or_default();
                write!(
                    f,
                    "{}:{}: code \"{}\" declared by err_code! again; first declared at {first_file}:{first_line} \
                     (declare it once and reference the const by path)",
                    self.file, self.line, self.code
                )
            }
            ViolationKind::BareLiteral => write!(
                f,
                "{}:{}: bare code literal \"{}\" after `{}`; declare it once and use the const: {}",
                self.file,
                self.line,
                self.code,
                self.context,
                self.template()
            ),
        }
    }
}

/// Every `*.rs` file under `dir`, sorted.
fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) -> std::io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_dir() {
            rust_files(&path, out)?;
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
    out.sort();
    Ok(())
}

fn well_formed(code: &str) -> bool {
    !code.is_empty() && code.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

/// 1-based line number of byte offset `at`.
fn line_of(text: &str, at: usize) -> usize {
    text[..at].bytes().filter(|b| *b == b'\n').count() + 1
}

/// True when the line holding `at` is a comment, or `at` follows a `//`.
fn in_comment(text: &str, at: usize) -> bool {
    let start = text[..at].rfind('\n').map_or(0, |i| i + 1);
    let before = &text[start..at];
    before.trim_start().starts_with("//") || before.contains("//")
}

/// Offset one past the string literal opening at `open` (a `"`).
fn skip_string(bytes: &[u8], open: usize) -> usize {
    let mut i = open + 1;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => i += 2,
            b'"' => return i + 1,
            _ => i += 1,
        }
    }
    bytes.len()
}

/// Offset of the delimiter closing the one opening at `open`, skipping
/// strings, char literals and comments.
fn matching_close(text: &str, open: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut depth = 0usize;
    let mut i = open;
    while i < bytes.len() {
        match bytes[i] {
            b'"' => {
                i = skip_string(bytes, i);
                continue;
            }
            b'/' if bytes.get(i + 1) == Some(&b'/') => {
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
                continue;
            }
            b'/' if bytes.get(i + 1) == Some(&b'*') => {
                i = text[i..].find("*/").map_or(bytes.len(), |e| i + e + 2);
                continue;
            }
            b'\'' if bytes.get(i + 2) == Some(&b'\'') => {
                i += 3;
                continue;
            }
            b'\'' if bytes.get(i + 1) == Some(&b'\\') && bytes.get(i + 3) == Some(&b'\'') => {
                i += 4;
                continue;
            }
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// Byte ranges of every `err_code! { .. }` invocation (not in a comment).
fn err_code_spans(text: &str) -> Vec<(usize, usize)> {
    let mut spans = Vec::new();
    let mut from = 0;
    while let Some(i) = text[from..].find("err_code!") {
        let at = from + i;
        from = at + "err_code!".len();
        if in_comment(text, at) {
            continue;
        }
        let rest = &text[from..];
        let open = from + (rest.len() - rest.trim_start().len());
        if matches!(text.as_bytes().get(open), Some(b'{' | b'(' | b'['))
            && let Some(close) = matching_close(text, open)
        {
            spans.push((open, close));
            from = close;
        }
    }
    spans
}

/// The literal (without quotes) that starts `rest` after whitespace, if any.
fn literal_after(rest: &str) -> Option<&str> {
    let lit = rest.trim_start().strip_prefix('"')?;
    lit.find('"').map(|end| &lit[..end])
}

/// Byte ranges of every `fn code(&self)` body.
fn code_fn_bodies(text: &str) -> Vec<(usize, usize)> {
    let mut bodies = Vec::new();
    let mut from = 0;
    while let Some(i) = text[from..].find("fn code(&self)") {
        let at = from + i;
        from = at + 1;
        if let Some(open) = text[at..].find('{').map(|o| at + o)
            && let Some(close) = matching_close(text, open)
        {
            bodies.push((open, close));
        }
    }
    bodies
}

fn format_violation(file: &str, text: &str, at: usize, context: &str) -> Violation {
    Violation {
        kind: ViolationKind::FormatBuilt,
        file: file.to_string(),
        line: line_of(text, at),
        code: String::new(),
        context: context.to_string(),
        first_site: None,
    }
}

/// The code an `err_code!` invocation body declares: the literal after
/// `code:` or after the arm form's `=>`, whichever comes first.
fn declared_code(body: &str) -> Option<&str> {
    let by_field = body.find(CODE_FIELD).and_then(|i| literal_after(&body[i + CODE_FIELD.len()..]).map(|c| (i, c)));
    let by_arrow = body.find("=>").and_then(|i| literal_after(&body[i + 2..]).map(|c| (i, c)));
    match (by_field, by_arrow) {
        (Some(a), Some(b)) => Some(if a.0 < b.0 { a.1 } else { b.1 }),
        (Some(a), None) | (None, Some(a)) => Some(a.1),
        (None, None) => None,
    }
}

/// One `err_code!` declaration site.
struct Decl {
    code: String,
    file: String,
    line: usize,
}

/// Scan one file's text; `file` is the display path.
fn scan_text(file: &str, text: &str, out: &mut Vec<Violation>, decls: &mut Vec<Decl>) {
    let spans = err_code_spans(text);
    for (open, close) in &spans {
        if let Some(code) = declared_code(&text[*open..*close]) {
            decls.push(Decl { code: code.to_string(), file: file.to_string(), line: line_of(text, *open) });
        }
    }
    let inside_decl = |at: usize| spans.iter().any(|(s, e)| at >= *s && at <= *e);
    for prefix in PREFIXES {
        let mut from = 0;
        while let Some(i) = text[from..].find(prefix) {
            let at = from + i;
            from = at + prefix.len();
            if in_comment(text, at) {
                continue;
            }
            if text[from..].trim_start().starts_with("format!") {
                out.push(format_violation(file, text, at, prefix));
                continue;
            }
            if inside_decl(at) {
                continue;
            }
            if let Some(code) = literal_after(&text[from..]).filter(|c| well_formed(c)) {
                out.push(Violation {
                    kind: ViolationKind::BareLiteral,
                    file: file.to_string(),
                    line: line_of(text, at),
                    code: code.to_string(),
                    context: prefix.to_string(),
                    first_site: None,
                });
            }
        }
    }
    for (open, close) in code_fn_bodies(text) {
        let body = &text[open..close];
        let mut from = 0;
        while let Some(i) = body[from..].find("=>") {
            let at = open + from + i;
            from += i + 2;
            if in_comment(text, at) {
                continue;
            }
            if text[at + 2..].trim_start().starts_with("format!") {
                out.push(format_violation(file, text, at, "AppError::code() arm"));
                continue;
            }
            if let Some(code) = literal_after(&text[at + 2..]).filter(|c| well_formed(c)) {
                out.push(Violation {
                    kind: ViolationKind::BareLiteral,
                    file: file.to_string(),
                    line: line_of(text, at),
                    code: code.to_string(),
                    context: "AppError::code() arm".to_string(),
                    first_site: None,
                });
            }
        }
    }
}

/// Scan every `*.rs` file under `root` and return the violations, ordered by
/// file then line. `root` is typically a crate's `src` directory.
pub fn scan_tree(root: &Path) -> std::io::Result<Vec<Violation>> {
    let mut files = Vec::new();
    rust_files(root, &mut files)?;
    let base = root.parent().unwrap_or(root);
    let mut out = Vec::new();
    let mut decls = Vec::new();
    for path in files {
        let text = std::fs::read_to_string(&path)?;
        let rel = path.strip_prefix(base).unwrap_or(&path).display().to_string();
        scan_text(&rel, &text, &mut out, &mut decls);
    }
    let mut first: std::collections::BTreeMap<&str, &Decl> = std::collections::BTreeMap::new();
    for d in &decls {
        match first.get(d.code.as_str()) {
            None => {
                first.insert(&d.code, d);
            }
            Some(f) => out.push(Violation {
                kind: ViolationKind::Duplicate,
                file: d.file.clone(),
                line: d.line,
                code: d.code.clone(),
                context: "err_code!".to_string(),
                first_site: Some((f.file.clone(), f.line)),
            }),
        }
    }
    out.sort_by(|a, b| (&a.file, a.line).cmp(&(&b.file, b.line)));
    Ok(out)
}

/// The `cargo test --lib` gate (PRD-mcphost-error-codes-generated-registry
/// R3): the real `src/` tree has no bare code literal, no `format!`-built
/// code and no code declared twice, and the linked registry has no duplicate.
#[cfg(test)]
mod gate {
    use super::scan_tree;
    use std::path::Path;

    #[test]
    fn src_tree_declares_every_code_once_with_err_code() {
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let found = match scan_tree(&src) {
            Ok(found) => found,
            Err(e) => panic!("scanner could not read {}: {e}", src.display()),
        };
        let lines: Vec<String> = found.iter().map(ToString::to_string).collect();
        assert!(found.is_empty(), "error-code gate failed:\n{}", lines.join("\n"));
    }

    #[test]
    fn linked_registry_has_no_duplicate_code() {
        let mut seen = std::collections::BTreeMap::new();
        for e in crate::error_registry::REGISTERED.iter() {
            if let Some(first) = seen.insert(e.code, e.site) {
                panic!("code {:?} registered at both {first} and {}", e.code, e.site);
            }
        }
        assert!(seen.len() > 150, "only {} codes registered", seen.len());
    }
}
