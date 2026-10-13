//! PRD-mcphost-funnel-by-surface-nightly-probe
//! AC2 (P0) — Given `SURFACES`, When the drift test renders the README
//! install section and the `host.quickstart` client list, Then every id
//! appears in both and no client id appears there that is not in
//! `SURFACES`.

use mcphost::install_links::{self, SURFACES};
use serde_json::json;
use std::collections::BTreeSet;

/// `**Label**` headings of the rendered README install section.
fn readme_labels(section: &str) -> BTreeSet<String> {
    section
        .lines()
        .filter_map(|l| l.strip_prefix("**").and_then(|l| l.strip_suffix("**")))
        .map(str::to_string)
        .collect()
}

#[test]
fn every_surface_id_is_in_the_readme_section_and_quickstart_list_and_nothing_else() {
    let base = install_links::CANONICAL_PUBLIC_URL;
    let links = install_links::for_url(base);

    // README install section: one `**Label**` block per surface; map each
    // SURFACES id to its label through the table and compare both ways.
    let rendered = install_links::render_readme_section(base);
    let labels = readme_labels(&rendered);
    let expected_labels: BTreeSet<String> = SURFACES
        .iter()
        .map(|id| {
            links
                .surfaces
                .iter()
                .find(|s| s.id == *id)
                .unwrap_or_else(|| panic!("SURFACES id {id} has no install row"))
                .label
                .to_string()
        })
        .collect();
    assert_eq!(labels, expected_labels, "README install section vs SURFACES");

    // `host.quickstart` client list: exactly the JSON the handler inserts.
    let quickstart = json!(links);
    let ids: BTreeSet<String> = quickstart["surfaces"]
        .as_array()
        .expect("surfaces array")
        .iter()
        .map(|s| s["id"].as_str().expect("id").to_string())
        .collect();
    let expected_ids: BTreeSet<String> = SURFACES.iter().map(|s| s.to_string()).collect();
    assert_eq!(ids, expected_ids, "quickstart client list vs SURFACES");

    // The prod `clientInfo.name` variants map onto SURFACES ids or `other`.
    let mapped: Vec<&str> = [
        Some("claude-code"),
        Some("Claude Code 2.0"),
        Some("cursor"),
        Some("Visual Studio Code"),
        Some("ChatGPT"),
        Some("claude-ai"),
        Some("codex-cli"),
        Some("unknown-client"),
        None,
        Some("gemini-cli"),
    ]
    .into_iter()
    .map(install_links::surface_id)
    .collect();
    assert_eq!(mapped.iter().filter(|m| SURFACES.contains(m)).count(), 8, "{mapped:?}");
    assert_eq!(mapped.iter().filter(|m| **m == "other").count(), 2, "{mapped:?}");
    for (prefix, id) in install_links::SURFACE_EXTRA_PREFIXES {
        assert!(SURFACES.contains(id), "extra prefix {prefix} -> {id} not in SURFACES");
    }
}
