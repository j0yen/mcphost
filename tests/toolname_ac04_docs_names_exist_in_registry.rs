//! PRD-mcphost-tool-naming-convention-and-aliases
//! AC4 (P0) -- Given `docs/**` and `plugin/**` at the landing commit, When
//! the docs-names test runs, Then every `host.*`/`billing.*`/`signup`
//! literal exists in the registry, and `docs/tools.md` is byte-identical
//! to a fresh `gen-docs` run.
//!
//! Only a markdown inline-code span (`` `exactly.this` ``) that FULLY
//! matches the tool-name shape counts as a literal -- a bare substring
//! match (`grep`-style) would false-positive on `` `mcphost.dev` `` /
//! `` `mcphost.db` `` / `` `mcp-host.md` `` (none of which are tool names,
//! all of which appear in these docs) the moment "host." shows up anywhere
//! inside a longer token.

use mcphost::kinds::KindRegistry;

fn repo_root() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// `docs/receipts/**` and `docs/benchmarks/**` are point-in-time
/// measurement snapshots, not promises about the live tool surface --
/// `scripts/docs-check.sh` already treats them as their own category
/// (valid-UTF8-and-non-empty only, no name-validity expectation), and a
/// receipt is free to describe a not-yet-shipped PRD's tool names (e.g. a
/// vault PRD's own receipt, landed separately from this one). Same
/// exclusion here, for the same reason.
fn is_excluded_dir(path: &std::path::Path) -> bool {
    path.components().any(|c| {
        let s = c.as_os_str().to_string_lossy();
        s == "receipts" || s == "benchmarks"
    })
}

fn md_files_under(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if is_excluded_dir(&path) {
            continue;
        }
        if path.is_dir() {
            md_files_under(&path, out);
        } else if path.extension().is_some_and(|e| e == "md") {
            out.push(path);
        }
    }
}

/// A backtick-delimited span that, taken whole, looks like a `host.*`/
/// `billing.*` tool name or is exactly `signup`.
fn looks_like_a_tool_literal(span: &str) -> bool {
    if span == "signup" {
        return true;
    }
    let Some(rest) = span.strip_prefix("host.").or_else(|| span.strip_prefix("billing.")) else {
        return false;
    };
    !rest.is_empty()
        && rest
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '.')
}

/// Fenced ```...``` code blocks are removed before the single-backtick
/// inline-span scan below -- otherwise a fence's own backtick characters
/// shift the odd/even parity `split('`').step_by(2)` relies on for every
/// span in the rest of the file.
fn strip_fenced_code_blocks(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("```") {
        out.push_str(&rest[..start]);
        rest = &rest[start + 3..];
        match rest.find("```") {
            Some(end) => rest = &rest[end + 3..],
            None => {
                rest = "";
                break;
            }
        }
    }
    out.push_str(rest);
    out
}

#[test]
fn every_host_billing_signup_literal_in_docs_and_plugin_exists_in_the_registry() {
    let kinds = KindRegistry::with_builtin();
    let known = mcphost::llms_txt::tenant_tool_names(&kinds);

    let mut files = Vec::new();
    md_files_under(&repo_root().join("docs"), &mut files);
    md_files_under(&repo_root().join("plugin"), &mut files);
    assert!(!files.is_empty(), "expected at least one docs/plugin markdown file");

    let mut missing: Vec<String> = Vec::new();
    for path in &files {
        let Ok(text) = std::fs::read_to_string(path) else { continue };
        let text = strip_fenced_code_blocks(&text);
        for span in text.split('`').skip(1).step_by(2) {
            if looks_like_a_tool_literal(span) && !known.iter().any(|n| n == span) {
                missing.push(format!("{}: `{span}`", path.display()));
            }
        }
    }
    assert!(missing.is_empty(), "literal(s) not in the live registry:\n{}", missing.join("\n"));
}

#[test]
fn docs_tools_md_is_byte_identical_to_a_fresh_gen_docs_run() {
    let kinds = KindRegistry::with_builtin();
    let rendered = mcphost::toolsdoc::render_tools_doc(&kinds);
    let committed = std::fs::read_to_string(repo_root().join("docs/tools.md")).expect("read docs/tools.md");
    assert_eq!(
        committed, rendered,
        "docs/tools.md is stale -- run `cargo run --bin mcphost -- tools-doc`"
    );
}
