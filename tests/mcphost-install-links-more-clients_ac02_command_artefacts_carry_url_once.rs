//! AC2 (PRD-mcphost-install-links-more-clients) — Given the table, When
//! each `command` artefact is inspected, Then it contains
//! `https://mcphost.dev/mcp` exactly once and no placeholder text.

use mcphost::install_links::{self, Kind};

#[test]
fn every_command_artefact_names_the_mcp_url_exactly_once_with_no_placeholder() {
    let surfaces = install_links::for_url("https://mcphost.dev");
    let commands: Vec<_> = surfaces.iter().filter(|s| s.kind == Kind::Command).collect();
    assert!(commands.len() >= 5, "expected at least five command rows: {commands:?}");

    for s in commands {
        assert_eq!(
            s.artefact.matches("https://mcphost.dev/mcp").count(),
            1,
            "{} must contain the MCP URL exactly once: {}",
            s.id,
            s.artefact
        );
        assert_eq!(s.artefact.lines().count(), 1, "{} must be a single pasteable line", s.id);
        let lower = s.artefact.to_lowercase();
        for placeholder in ["<", ">", "{", "}", "your", "xxx", "todo", "example.com", "<key>"] {
            assert!(
                !lower.contains(placeholder),
                "{} carries placeholder text {placeholder:?}: {}",
                s.id,
                s.artefact
            );
        }
    }
}
