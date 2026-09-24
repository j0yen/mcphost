//! AC3 (PRD-mcphost-claude-code-plugin-and-snippets) — Given README.md,
//! When grepped, Then a `## Connect` heading exists and
//! `https://mcphost.dev/mcp` appears at least four times below it, once
//! each within blocks mentioning Claude Code, Codex, Cursor, and
//! Claude.ai.

const README: &str = include_str!("../README.md");
const URL: &str = "https://mcphost.dev/mcp";

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
fn connect_heading_exists() {
    assert!(README.contains("## Connect"));
}

#[test]
fn url_appears_at_least_four_times_in_connect_section() {
    let section = connect_section();
    let count = section.matches(URL).count();
    assert!(
        count >= 4,
        "expected >=4 occurrences of {URL} under '## Connect', found {count}"
    );
}

#[test]
fn each_client_block_mentions_the_url() {
    let section = connect_section();
    for label in ["Claude Code", "Codex", "Cursor", "Claude.ai"] {
        let label_idx = section
            .find(label)
            .unwrap_or_else(|| panic!("'## Connect' section must mention {label}"));
        let window_end = (label_idx + 600).min(section.len());
        let window = &section[label_idx..window_end];
        assert!(
            window.contains(URL),
            "the {label} block must contain {URL} within a reasonable distance of its label"
        );
    }
}
