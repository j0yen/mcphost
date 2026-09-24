//! AC4 (PRD-mcphost-claude-code-plugin-and-snippets) — Given
//! `www/index.html`, When grepped, Then the four snippets are present and
//! the page loads with no external script tags added by this change.

const INDEX_HTML: &str = include_str!("../www/index.html");

#[test]
fn four_client_snippets_are_present() {
    assert!(
        INDEX_HTML.contains("claude mcp add --transport http mcphost https://mcphost.dev/mcp"),
        "index.html must contain the Claude Code snippet"
    );
    assert!(
        INDEX_HTML.contains("codex mcp add mcphost --url https://mcphost.dev/mcp"),
        "index.html must contain the Codex CLI snippet"
    );
    assert!(
        INDEX_HTML.contains("\"url\": \"https://mcphost.dev/mcp\""),
        "index.html must contain the Cursor mcp.json snippet"
    );
    assert!(
        INDEX_HTML.contains("Add custom connector"),
        "index.html must contain the Claude.ai custom connector snippet"
    );
}

#[test]
fn no_external_script_tags() {
    for line in INDEX_HTML.lines() {
        let lower = line.to_ascii_lowercase();
        if lower.contains("<script") {
            assert!(
                !lower.contains("src="),
                "index.html must not add an external <script src=...> tag: {line}"
            );
        }
    }
}
