//! PRD-mcphost-kind-field-docs-required-from-parse-spec
//! AC6 (P1) — Given gendocs runs, When the kind page is rendered, Then a
//! generated "Minimal specs" section carries one JSON block per
//! `minimal_specs()` entry and the hsr fixtures match.

use mcphost::kinds::production_registry;
use serde_json::Value;

const START: &str = "<!-- kind-minimal-specs:start -->";
const END: &str = "<!-- kind-minimal-specs:end -->";
const FIELDS_START: &str = "<!-- kind-fields:start -->";
const FIELDS_END: &str = "<!-- kind-fields:end -->";

fn root() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn between<'a>(text: &'a str, start: &str, end: &str) -> Option<&'a str> {
    let s = text.find(start)? + start.len();
    let e = text.find(end)?;
    (s <= e).then(|| &text[s..e])
}

/// The JSON values of the fenced ```json blocks in `block`, in order.
fn json_blocks(block: &str) -> Vec<Value> {
    block
        .split("```json\n")
        .skip(1)
        .map(|rest| {
            let body = rest.split("\n```").next().expect("closing fence");
            serde_json::from_str(body).expect("a fenced block is JSON")
        })
        .collect()
}

#[test]
fn kreqd_ac06_minimal_specs_section() {
    let registry = production_registry();
    for name in registry.names() {
        let kind = registry.get(name).expect("registered kind resolves");
        let page = std::fs::read_to_string(root().join(format!("docs/kinds/{name}.md"))).expect("kind page");
        assert!(page.contains("## Minimal specs"), "kind {name}: page has no Minimal specs section");
        let block = between(&page, START, END).unwrap_or_else(|| panic!("kind {name}: no kind-minimal-specs markers"));
        assert_eq!(json_blocks(block), kind.minimal_specs(), "kind {name}: Minimal specs blocks != minimal_specs()");

        // The frozen hsr corpus copy of this page (when it carries one) agrees.
        let fixture = root().join(format!("tests/fixtures/hsr_corpus/docs/kinds/{name}.md.fixture"));
        let Ok(frozen) = std::fs::read_to_string(&fixture) else { continue };
        assert_eq!(
            between(&frozen, START, END),
            Some(block),
            "kind {name}: hsr fixture Minimal specs block differs from docs/kinds/{name}.md"
        );
        assert_eq!(
            between(&frozen, FIELDS_START, FIELDS_END),
            between(&page, FIELDS_START, FIELDS_END),
            "kind {name}: hsr fixture kind-fields table differs from docs/kinds/{name}.md"
        );
    }
}
