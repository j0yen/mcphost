//! PRD-mcphost-table-import-url-live-proof
//! AC2 (P0) -- Given every absolute example-fixture URL (`/examples/<file>` on
//! the public origin) found under `agent/`, `recipes/`, `www/`, `docs/` and
//! `examples/`, When the route drift test runs, Then each path is a route in
//! the binary's static-route table, and the test fails on a URL that has none.
//!
//! The static-route table is `http::routes::UNSPECCED` (the compile-time
//! embedded pages) plus `ROUTES`; `oapi_ac02` already proves every route
//! `build_router` mounts is in one of the two.

use mcphost::http::routes::{ROUTES, UNSPECCED};
use std::collections::BTreeSet;
use std::path::Path;

const ORIGIN: &str = "https://mcphost.dev";
const SCAN_DIRS: &[&str] = &["agent", "recipes", "www", "docs", "examples"];

/// Every `<ORIGIN>/examples/<file>` path in `text`, origin stripped. A URL
/// ends at whitespace, a quote, a bracket, a backtick, a comma or a
/// semicolon; a trailing sentence `.` or `:` is not part of it.
pub fn example_paths(text: &str) -> Vec<String> {
    let needle = format!("{ORIGIN}/examples/");
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(i) = rest.find(&needle) {
        let after = &rest[i + ORIGIN.len()..];
        let end = after
            .find(|c: char| c.is_whitespace() || "\"'`<>()[]{},;\\|*".contains(c))
            .unwrap_or(after.len());
        let path = after[..end].trim_end_matches(['.', ':', '!', '?']);
        // Drop a query or fragment: the route table keys on the path alone.
        let path = path.split(['?', '#']).next().unwrap_or(path);
        out.push(path.to_string());
        rest = &rest[i + needle.len()..];
    }
    out
}

fn route_paths() -> BTreeSet<&'static str> {
    ROUTES.iter().map(|r| r.path).chain(UNSPECCED.iter().copied()).collect()
}

/// `(file, path)` for each advertised example URL with no route in `known`.
pub fn unrouted(files: &[(String, String)], known: &BTreeSet<&str>) -> Vec<(String, String)> {
    let mut missing = Vec::new();
    for (file, text) in files {
        for path in example_paths(text) {
            if !known.contains(path.as_str()) {
                missing.push((file.clone(), path));
            }
        }
    }
    missing
}

fn collect(dir: &Path, root: &Path, out: &mut Vec<(String, String)>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect(&path, root, out);
        } else if let Ok(text) = std::fs::read_to_string(&path) {
            let rel = path.strip_prefix(root).unwrap_or(&path).display().to_string();
            out.push((rel, text));
        }
    }
}

fn scanned_files() -> Vec<(String, String)> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut out = Vec::new();
    for dir in SCAN_DIRS {
        collect(&root.join(dir), root, &mut out);
    }
    out
}

#[test]
fn tblurlp_ac02_every_advertised_example_url_is_a_static_route() {
    let known = route_paths();
    assert!(
        known.contains("/examples/sample-1000.csv"),
        "the database-in-a-minute fixture must be in the static-route table"
    );
    let files = scanned_files();
    assert!(!files.is_empty(), "the scan found no files under {SCAN_DIRS:?}");
    let missing = unrouted(&files, &known);
    assert!(
        missing.is_empty(),
        "advertised example URLs with no route in the binary (they 404 on prod): {missing:?}"
    );
}

#[test]
fn tblurlp_ac02_a_url_without_a_route_fails_the_check() {
    let known = route_paths();
    let files = vec![
        (
            "synthetic.md".to_string(),
            "fetch https://mcphost.dev/examples/no-such-fixture.csv. Then\nuse `https://mcphost.dev/examples/sample-1000.csv`.".to_string(),
        ),
        ("other.md".to_string(), "see https://example.com/examples/x.csv, not ours".to_string()),
    ];
    let missing = unrouted(&files, &known);
    assert_eq!(
        missing,
        vec![("synthetic.md".to_string(), "/examples/no-such-fixture.csv".to_string())],
        "only the unrouted mcphost.dev URL is reported; the routed one and the foreign origin pass"
    );
}
