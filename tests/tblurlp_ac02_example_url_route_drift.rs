//! PRD-mcphost-table-import-url-live-proof
//! AC2 (P0) -- Given every absolute example-fixture URL
//! (`https://mcphost.dev/examples/<file>`) found under `agent/`, `recipes/`,
//! `www/`, `docs/` and `examples/`, When the route drift test runs, Then each
//! path is a route in the binary's static-route table, and the test fails on a
//! URL that has none.
//!
//! The static-route table is `http::routes::UNSPECCED` (the compile-time
//! embedded `www/*` pages) plus the spec'd `ROUTES`. `agent/intent-card.json`
//! is scanned like every other file: agents read the card, so an AC text that
//! advertises a URL is advertising it.

use mcphost::http::routes::{ROUTES, UNSPECCED};
use regex::Regex;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

pub const SCANNED_DIRS: [&str; 5] = ["agent", "recipes", "www", "docs", "examples"];

fn example_url_re() -> Regex {
    Regex::new(r"https://mcphost\.dev(/examples/[A-Za-z0-9._~/%-]*[A-Za-z0-9_~%/-])").expect("regex")
}

/// Every `/examples/<file>` path advertised on the public origin in `text`.
pub fn advertised_paths(text: &str) -> BTreeSet<String> {
    example_url_re().captures_iter(text).map(|c| c[1].to_string()).collect()
}

fn static_route_table() -> BTreeSet<String> {
    ROUTES.iter().map(|r| r.path).chain(UNSPECCED.iter().copied()).map(str::to_string).collect()
}

/// Advertised paths in `text` that no route in `table` serves.
pub fn unrouted(text: &str, table: &BTreeSet<String>) -> Vec<String> {
    advertised_paths(text).into_iter().filter(|p| !table.contains(p)).collect()
}

fn files_under(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            files_under(&path, out);
        } else {
            out.push(path);
        }
    }
}

/// `(file, path)` for every advertised example URL under the scanned dirs.
pub fn advertised_in_tree() -> Vec<(PathBuf, String)> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut files = Vec::new();
    for dir in SCANNED_DIRS {
        files_under(&root.join(dir), &mut files);
    }
    files.sort();
    let mut out = Vec::new();
    for file in files {
        let Ok(text) = std::fs::read_to_string(&file) else { continue };
        for path in advertised_paths(&text) {
            out.push((file.strip_prefix(root).unwrap_or(&file).to_path_buf(), path));
        }
    }
    out
}

#[test]
fn tblurlp_ac02_every_advertised_example_url_is_a_static_route() {
    let table = static_route_table();
    let missing: Vec<String> = advertised_in_tree()
        .into_iter()
        .filter(|(_, path)| !table.contains(path))
        .map(|(file, path)| format!("{} advertises https://mcphost.dev{path} but no static route serves it", file.display()))
        .collect();
    assert!(missing.is_empty(), "example URLs without a route:\n{}", missing.join("\n"));
}

#[test]
fn tblurlp_ac02_the_fixture_path_is_in_the_static_route_table() {
    assert!(static_route_table().contains("/examples/sample-1000.csv"));
}

#[test]
fn tblurlp_ac02_a_url_with_no_route_fails_naming_it() {
    let table = static_route_table();
    let text = "see https://mcphost.dev/examples/sample-1000.csv and (https://mcphost.dev/examples/nope-2000.csv).";
    assert_eq!(advertised_paths(text).len(), 2, "both URLs are found, the trailing `).` is not part of the path");
    assert_eq!(unrouted(text, &table), vec!["/examples/nope-2000.csv".to_string()]);
    assert!(unrouted("https://mcphost.dev/llms.txt https://example.com/examples/x.csv", &table).is_empty());
}
