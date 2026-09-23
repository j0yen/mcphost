//! AC9 (PRD-mcphost-signup-kill-switch-and-source) — Given www/llms.txt on
//! the built tree, When grepped, Then it documents `source` and lists
//! `hn` and `registry`.

const LLMS_TXT: &str = include_str!("../www/llms.txt");

const SECTION_HEADING: &str = "## Quickstart for agents";
const NEXT_HEADING: &str = "## Contributing: routing a new top-level path";

fn section() -> &'static str {
    let start = LLMS_TXT
        .find(SECTION_HEADING)
        .unwrap_or_else(|| panic!("www/llms.txt must have a '{SECTION_HEADING}' section"));
    let end = LLMS_TXT[start..]
        .find(NEXT_HEADING)
        .map(|offset| start + offset)
        .unwrap_or(LLMS_TXT.len());
    &LLMS_TXT[start..end]
}

#[test]
fn quickstart_documents_the_source_argument() {
    let section = section();
    assert!(
        section.contains("`source`"),
        "the quickstart section must document signup's `source` argument"
    );
    assert!(
        section.contains("signup(name, source:"),
        "the quickstart section must show a signup(...source:...) example"
    );
}

#[test]
fn quickstart_lists_hn_and_registry_as_recommended_sources() {
    let section = section();
    assert!(section.contains("`hn`"), "recommended sources must list hn");
    assert!(
        section.contains("`registry`"),
        "recommended sources must list registry"
    );
}
