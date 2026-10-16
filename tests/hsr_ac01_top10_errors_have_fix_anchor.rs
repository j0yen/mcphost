//! PRD-mcphost-help-search-tool
//! AC1 (P0) — Given `tests/fixtures/hsr_top10_errors.json` (the 10 most
//! frequent 10-08 explore error strings, each with its fix anchor), When each
//! string is sent as `q` with `k=3`, Then >= 8 of 10 responses contain the fix
//! anchor; a lower count fails the test with the misses listed.
//!
//! The ranking test indexes `tests/fixtures/hsr_corpus/`, a frozen copy of the
//! docs, not the live tree: the live index is rebuilt from `docs/` at compile
//! time, so a PRD branch that adds a page or a table row moves the scores and
//! used to flip this test with no change to the search (three CI failures
//! 10-15, `unknown_import` gaining a row in python.md's error table).
//! `every_fixture_anchor_exists_in_the_live_docs` keeps the live side honest:
//! a renamed heading still fails here.

use mcphost::help_search::{HELP_DOCS, HelpIndex};
use mcphost::kinds::KindRegistry;
use serde_json::Value;

pub(crate) fn fixture() -> Vec<(String, String)> {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/hsr_top10_errors.json");
    let rows: Vec<Value> = serde_json::from_str(&std::fs::read_to_string(path).expect("fixture readable"))
        .expect("fixture is a JSON array");
    rows.iter()
        .map(|r| (r["error"].as_str().expect("error").to_string(), r["anchor"].as_str().expect("anchor").to_string()))
        .collect()
}

/// The frozen corpus as `(repo-relative path, markdown)`: every
/// `*.fixture` file under `tests/fixtures/hsr_corpus/`, path minus the suffix.
fn frozen_docs() -> Vec<(String, String)> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/hsr_corpus");
    let mut out = Vec::new();
    let mut stack = vec![root.clone()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).expect("corpus dir readable").filter_map(Result::ok) {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if let Some(rel) = path.strip_prefix(&root).ok().and_then(|r| r.to_str()).and_then(|r| r.strip_suffix(".fixture")) {
                out.push((rel.replace('\\', "/"), std::fs::read_to_string(&path).expect("corpus file readable")));
            }
        }
    }
    out.sort();
    out
}

#[test]
fn at_least_eight_of_ten_errors_return_their_fix_anchor_in_top3() {
    let docs = frozen_docs();
    assert!(docs.len() >= 30, "the frozen corpus is the whole docs tree, found {} pages", docs.len());
    let borrowed: Vec<(&str, &str)> = docs.iter().map(|(p, t)| (p.as_str(), t.as_str())).collect();
    let index = HelpIndex::build_from_docs(&borrowed, &KindRegistry::with_builtin());
    let rows = fixture();
    assert_eq!(rows.len(), 10, "the fixture is the ten most frequent errors");

    let mut misses = Vec::new();
    for (error, anchor) in &rows {
        let (hits, _total) = index.search(error, 3, None);
        let anchors: Vec<&str> = hits.iter().map(|h| h.anchor.as_str()).collect();
        if !anchors.contains(&anchor.as_str()) {
            misses.push(format!("{error:?}: want {anchor}, got {anchors:?}"));
        }
    }
    assert!(
        rows.len() - misses.len() >= 8,
        "only {}/10 error strings returned their fix anchor in the top 3; misses:\n{}",
        rows.len() - misses.len(),
        misses.join("\n")
    );
}

/// The ranking is a pure function of the corpus: two builds over the same
/// docs answer every fixture query with the same anchors in the same order.
#[test]
fn ranking_is_deterministic_across_index_builds() {
    let docs = frozen_docs();
    let borrowed: Vec<(&str, &str)> = docs.iter().map(|(p, t)| (p.as_str(), t.as_str())).collect();
    let kinds = KindRegistry::with_builtin();
    let first = HelpIndex::build_from_docs(&borrowed, &kinds);
    for _ in 0..5 {
        let again = HelpIndex::build_from_docs(&borrowed, &kinds);
        for (error, _) in fixture() {
            let a: Vec<String> = first.search(&error, 5, None).0.into_iter().map(|h| h.anchor).collect();
            let b: Vec<String> = again.search(&error, 5, None).0.into_iter().map(|h| h.anchor).collect();
            assert_eq!(a, b, "ranking for {error:?} moved between builds");
        }
    }
}

/// Every fix anchor the fixture names still exists in the live docs (or is a
/// tool): the check the frozen corpus cannot make.
#[test]
fn every_fixture_anchor_exists_in_the_live_docs() {
    let index = HelpIndex::build_from_docs(HELP_DOCS, &KindRegistry::with_builtin());
    let missing: Vec<String> =
        fixture().into_iter().map(|(_, anchor)| anchor).filter(|a| !index.has_anchor(a)).collect();
    assert!(missing.is_empty(), "fixture fix anchors absent from the live docs index: {missing:?}");
}
