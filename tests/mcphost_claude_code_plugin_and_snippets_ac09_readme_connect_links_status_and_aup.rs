//! AC9 (PRD-mcphost-claude-code-plugin-and-snippets) — Given README
//! "Connect", When grepped, Then it links to `/status.html` and
//! `/aup.html`.

const README: &str = include_str!("../README.md");

fn connect_section() -> &'static str {
    let start = README
        .find("## Connect")
        .unwrap_or_else(|| panic!("README.md must have a '## Connect' heading"));
    let after_heading = start + "## Connect".len();
    let end = README[after_heading..]
        .find("\n## ")
        .map(|offset| after_heading + offset)
        .unwrap_or(README.len());
    &README[start..end]
}

#[test]
fn connect_section_links_status_page() {
    assert!(
        connect_section().contains("/status.html"),
        "README '## Connect' section must link to /status.html"
    );
}

#[test]
fn connect_section_links_aup_page() {
    assert!(
        connect_section().contains("/aup.html"),
        "README '## Connect' section must link to /aup.html"
    );
}
