//! PRD-mcphost-capabilities-generated-from-dispatch
//! AC3 (P0) -- Given `src/`, When `grep -rn 'Mcp-Method\|mcp-method'
//! src/http.rs src/capabilities.rs src/handler.rs` and `grep -rn
//! 'capability_unsupported(' src/` run, Then both print nothing,
//! `enrich_unsupported_errors` and the per-hook handler overrides are absent,
//! and `unsupported_error` has exactly one call site outside tests.

use std::path::{Path, PathBuf};

fn root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

fn rs_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for e in std::fs::read_dir(dir).expect("read_dir") {
        let p = e.expect("entry").path();
        if p.is_dir() {
            rs_files(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
}

fn read(rel: &str) -> String {
    std::fs::read_to_string(root().join(rel)).unwrap_or_else(|e| panic!("read {rel}: {e}"))
}

/// The file's text before its trailing `#[cfg(test)]` module.
fn non_test_part(text: &str) -> &str {
    text.split("\n#[cfg(test)]").next().unwrap_or(text)
}

#[test]
fn cgd_ac03_no_header_keyed_refusal_and_no_per_hook_override() {
    for rel in ["src/http.rs", "src/capabilities.rs", "src/handler.rs"] {
        let text = read(rel);
        for needle in ["Mcp-Method", "mcp-method"] {
            assert!(!text.contains(needle), "{rel} still mentions `{needle}`");
        }
    }
    let mut files = Vec::new();
    rs_files(&root().join("src"), &mut files);
    for f in files {
        let text = std::fs::read_to_string(&f).expect("read");
        assert!(!text.contains("capability_unsupported("), "{} still calls capability_unsupported(", f.display());
    }
    let http = read("src/http.rs");
    assert!(!http.contains("enrich_unsupported_errors"), "enrich_unsupported_errors is back");
    let handler = read("src/handler.rs");
    // A row that is implemented has real handlers (`list_prompts` / `get_prompt`
    // since mcphost-prompts-from-published-tools, the `resources/*` hooks since
    // mcphost-resources-for-tenant-data), not refusal overrides: only the hooks
    // of rows still `implemented: false` must be absent.
    let hooks: [(&str, &[&str]); 3] = [
        ("resources", &["list_resources", "list_resource_templates", "read_resource", "subscribe", "unsubscribe"]),
        ("completions", &["complete"]),
        ("logging", &["set_level"]),
    ];
    for (row, names) in hooks {
        let implemented = mcphost::capabilities::CAPABILITIES.iter().any(|r| r.name == row && r.implemented);
        if implemented {
            continue;
        }
        for hook in names {
            assert!(!handler.contains(&format!("async fn {hook}(")), "handler.rs still overrides `{hook}`");
        }
    }
}

#[test]
fn cgd_ac03_unsupported_error_has_exactly_one_call_site_outside_tests() {
    let mut files = Vec::new();
    rs_files(&root().join("src"), &mut files);
    let mut sites = Vec::new();
    for f in files {
        let text = std::fs::read_to_string(&f).expect("read");
        for (i, line) in non_test_part(&text).lines().enumerate() {
            let code = line.split("//").next().unwrap_or("");
            if code.contains("unsupported_error(") && !code.contains("fn unsupported_error(") {
                sites.push(format!("{}:{}", f.display(), i + 1));
            }
        }
    }
    assert_eq!(sites.len(), 1, "unsupported_error call sites: {sites:?}");
}
