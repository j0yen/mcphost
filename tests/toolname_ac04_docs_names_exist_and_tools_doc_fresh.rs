//! PRD-mcphost-tool-naming-convention-and-aliases
//! AC4 — Given `docs/**` and `plugin/**` at the landing commit, When the
//! docs-names test runs, Then every `host.*`/`billing.*`/`signup` literal
//! exists in the registry, and `docs/tools.md` is byte-identical to a
//! fresh `gen-docs` run.

use mcphost::gendocs::render_tools_markdown;
use mcphost::kinds::KindRegistry;
use std::collections::HashSet;
use std::path::Path;

/// A `host.<...>`/`billing.<...>` literal (one or more dotted lowercase/
/// underscore segments after the prefix) or the bare word `signup` --
/// `admin.*` is out of scope (PRD non-goal: "the admin.* surface ...
/// separate rule"); `host.*`/`billing.*` wildcard mentions (e.g.
/// "host.state.* tools") are discarded, not reported as the truncated
/// prefix before the `*`; and a boundary check before the prefix excludes
/// a false match inside a longer word (`mcphost.call`, `mcphost.state` --
/// the python kind's own SDK calls, not a `host.*` tool -- would otherwise
/// read as `host.call`/`host.state`).
fn extract_tool_names(text: &str) -> Vec<String> {
    let mut names = Vec::new();
    let bytes: Vec<char> = text.chars().collect();
    let mut i = 0;
    while i < bytes.len() {
        for prefix in ["host.", "billing."] {
            let boundary_ok = i == 0 || !is_ident_char(bytes[i - 1]);
            if boundary_ok
                && text[byte_index(&bytes, i)..].starts_with(prefix)
                && bytes.get(i + prefix.chars().count()).is_some_and(|c| c.is_ascii_lowercase())
            {
                let start = i;
                let mut j = i + prefix.chars().count();
                while j < bytes.len() && (bytes[j].is_ascii_lowercase() || bytes[j].is_ascii_digit() || bytes[j] == '_' || bytes[j] == '.')
                {
                    j += 1;
                }
                // A wildcard mention ("host.state.* tools") -- not a real
                // name, and not the truncated prefix before the `*`.
                let is_wildcard = bytes.get(j) == Some(&'*');
                if !is_wildcard {
                    // Trim a trailing dot (e.g. "host.tool.share." at a
                    // sentence's end) -- never a real tool-name character.
                    let mut end = j;
                    while end > start && bytes[end - 1] == '.' {
                        end -= 1;
                    }
                    let name: String = bytes[start..end].iter().collect();
                    names.push(name);
                }
                i = j;
                break;
            }
        }
        // `signup` as a bare word (not part of a longer identifier).
        if text[byte_index(&bytes, i)..].starts_with("signup")
            && (i == 0 || !is_ident_char(bytes[i - 1]))
            && !bytes.get(i + 6).is_some_and(|&c| is_ident_char(c))
        {
            names.push("signup".to_string());
        }
        i += 1;
    }
    names
}

fn is_ident_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '.'
}

fn byte_index(chars: &[char], char_idx: usize) -> usize {
    chars[..char_idx].iter().map(|c| c.len_utf8()).sum()
}

fn markdown_files(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            markdown_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "md") {
            out.push(path);
        }
    }
}

#[test]
fn every_host_billing_signup_literal_in_docs_and_plugin_exists_in_the_registry() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let kinds = KindRegistry::with_builtin();
    let registry_names: HashSet<String> = mcphost::handler::host_tool_descriptors(&kinds)
        .into_iter()
        .map(|t| t.name.to_string())
        .chain(std::iter::once("signup".to_string()))
        .collect();

    let mut files = Vec::new();
    markdown_files(&dir.join("docs"), &mut files);
    markdown_files(&dir.join("plugin"), &mut files);
    assert!(!files.is_empty(), "expected to find markdown files under docs/ and plugin/");

    let mut missing: Vec<(String, std::path::PathBuf)> = Vec::new();
    for path in &files {
        let text = std::fs::read_to_string(path).unwrap_or_default();
        for name in extract_tool_names(&text) {
            // `host.*`/`billing.*` -- the top-level exception itself
            // mentioned as a wildcard/prose category, not a specific
            // tool. `extract_tool_names` already excludes literal `*`,
            // but a trailing bare prefix ("host." with nothing after, from
            // a sentence like "every host. tool") can't occur given the
            // `[a-z]` lookahead above; nothing to special-case here.
            if !registry_names.contains(&name) {
                missing.push((name, path.clone()));
            }
        }
    }
    assert!(
        missing.is_empty(),
        "the following docs/plugin literals name no registry tool (canonical or alias): {missing:?}"
    );
}

#[test]
fn docs_tools_md_is_byte_identical_to_a_fresh_gen_docs_run() {
    let committed = include_str!("../docs/tools.md");
    let fresh = render_tools_markdown(&KindRegistry::with_builtin());
    assert_eq!(
        committed, fresh,
        "docs/tools.md has drifted from the live registry -- run `mcphost gen-docs`"
    );
}

#[test]
fn extract_tool_names_finds_dotted_and_underscored_literals_but_not_wildcards() {
    let text = "Call host.tool.share then host.tool_share; host.* and billing.* are wildcards; \
                also signup and billing.plans; resignup is not signup.";
    let names = extract_tool_names(text);
    assert!(names.contains(&"host.tool.share".to_string()));
    assert!(names.contains(&"host.tool_share".to_string()));
    assert!(names.contains(&"billing.plans".to_string()));
    assert_eq!(names.iter().filter(|n| *n == "signup").count(), 1, "{names:?}");
    assert!(!names.iter().any(|n| n == "host." || n == "billing." || n == "host" || n == "billing"));
}
