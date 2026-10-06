//! AC1 (PRD-mcphost-client-install-links) — Given
//! `MCPHOST_PUBLIC_URL=https://mcphost.dev`, When `install_links::for_url`
//! runs, Then the Cursor link, VS Code link, Claude Code command, and
//! Claude.ai steps match the pinned fixtures byte for byte.
//!
//! Fixtures pinned against each client's own install-link doc, same as
//! `install_links.rs`'s own module doc comment:
//! - Cursor: https://docs.cursor.com/en/tools/mcp (checked 2026-10-06)
//! - VS Code: https://code.visualstudio.com/api/extension-guides/ai/mcp
//!   (checked 2026-10-06)
//! - Claude Code: https://code.claude.com/docs/en/mcp (checked 2026-10-06)

use mcphost::install_links;

const BASE: &str = "https://mcphost.dev";

const CURSOR_FIXTURE: &str = "cursor://anysphere.cursor-deeplink/mcp/install?name=mcphost&config=eyJ1cmwiOiJodHRwczovL21jcGhvc3QuZGV2L21jcCJ9";

const VSCODE_FIXTURE: &str = "vscode:mcp/install?%7B%22name%22%3A%22mcphost%22%2C%22type%22%3A%22http%22%2C%22url%22%3A%22https%3A%2F%2Fmcphost%2Edev%2Fmcp%22%7D";

const CLAUDE_CODE_FIXTURE: &str = "claude mcp add --transport http mcphost https://mcphost.dev/mcp";

const CLAUDE_AI_STEPS_FIXTURE: &[&str] = &[
    "Open Settings, then Connectors, then Add custom connector.",
    "Name: mcphost",
    "Remote MCP server URL: https://mcphost.dev/mcp",
    "Save, then enable the connector in a chat to connect.",
];

#[test]
fn for_url_matches_every_pinned_fixture_byte_for_byte() {
    let links = install_links::for_url(BASE);

    assert_eq!(links.mcp_url, "https://mcphost.dev/mcp", "{links:?}");
    assert_eq!(links.cursor, CURSOR_FIXTURE, "{links:?}");
    assert_eq!(links.vscode, VSCODE_FIXTURE, "{links:?}");
    assert_eq!(links.claude_code_command, CLAUDE_CODE_FIXTURE, "{links:?}");
    assert_eq!(links.claude_ai_steps, CLAUDE_AI_STEPS_FIXTURE, "{links:?}");
}

/// A trailing slash on `base` must not change any form -- same tolerance
/// `control::key_rotate`'s own `{base}/mcp` endpoint has.
#[test]
fn a_trailing_slash_on_base_is_tolerated() {
    let with_slash = install_links::for_url("https://mcphost.dev/");
    let without_slash = install_links::for_url(BASE);
    assert_eq!(with_slash, without_slash);
}
