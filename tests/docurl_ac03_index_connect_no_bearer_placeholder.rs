//! PRD-mcphost-docs-one-url-flow
//! AC3 (P0) -- Given the index connect section, When rendered, Then no
//! snippet contains a Bearer header placeholder and the one-sentence note
//! about the first call appears.

const INDEX_HTML: &str = include_str!("../www/index.html");

const CONNECT_HEADING: &str = "<h2>connect</h2>";
const NEXT_HEADING: &str = "<h2>one night on mcphost</h2>";

fn connect_section() -> &'static str {
    let start = INDEX_HTML
        .find(CONNECT_HEADING)
        .expect("www/index.html must have a 'connect' section");
    let end = INDEX_HTML[start..]
        .find(NEXT_HEADING)
        .map(|offset| start + offset)
        .expect("a heading must follow the connect section");
    &INDEX_HTML[start..end]
}

#[test]
fn no_snippet_in_the_connect_section_carries_a_bearer_placeholder() {
    let section = connect_section();
    assert!(
        !section.to_ascii_lowercase().contains("bearer"),
        "the connect section must carry no Bearer header placeholder: {section}"
    );
}

#[test]
fn the_first_call_note_appears_in_the_connect_section() {
    let section = connect_section();
    assert!(
        section.contains("your agent's first call creates its tenant")
            && section.contains("private URL to keep"),
        "the connect section must carry the one-sentence first-call note: {section}"
    );
}

#[test]
fn all_four_client_snippets_still_present_with_no_header() {
    let section = connect_section();
    assert!(section.contains("claude mcp add --transport http mcphost https://mcphost.dev/mcp"));
    assert!(section.contains("codex mcp add mcphost --url https://mcphost.dev/mcp"));
    assert!(section.contains("\"url\": \"https://mcphost.dev/mcp\""));
    assert!(!section.contains("\"headers\""), "the Cursor snippet must carry no headers object");
    assert!(section.contains("Add custom connector"));
}
