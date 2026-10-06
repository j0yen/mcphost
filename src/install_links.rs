//! `install_links::for_url` (PRD-mcphost-client-install-links P0
//! requirement 1, AC1): the four client install forms -- Cursor deep link,
//! VS Code deep link, Claude Code command, Claude.ai steps -- built from
//! one base URL, so `/connect` (`http.rs`), `host_quickstart`
//! (`control.rs`), and the README's generated block (below) can never
//! drift from each other or from the deployed hostname.
//!
//! Deep-link formats, each pinned against the client's own docs per the
//! PRD's technical considerations ("keep them in one module with fixtures
//! and a comment per client naming the doc page and the date checked"):
//! - **Cursor**: `cursor://anysphere.cursor-deeplink/mcp/install?name=<name>&config=<base64 json>`,
//!   config `{"url": "<mcp_url>"}` -- https://docs.cursor.com/en/tools/mcp
//!   (checked 2026-10-06).
//! - **VS Code**: `vscode:mcp/install?<percent-encoded json>`, json
//!   `{"name": "<name>", "type": "http", "url": "<mcp_url>"}` --
//!   https://code.visualstudio.com/api/extension-guides/ai/mcp (checked
//!   2026-10-06).
//! - **Claude Code**: `claude mcp add --transport http <name> <mcp_url>` --
//!   https://code.claude.com/docs/en/mcp (checked 2026-10-06).
//! - **Claude.ai**: no deep link exists (Open question in the PRD is still
//!   unresolved as of this writing) -- numbered steps through Settings ->
//!   Connectors -> Add custom connector, naming the exact URL to paste.

use base64::Engine as _;
use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};
use serde::Serialize;
use serde_json::json;

/// The display name every install form uses for this server -- so a
/// reader never has to type a name into the client's own "name" field by
/// hand, and every form names the same server.
pub const CLIENT_NAME: &str = "mcphost";

/// The hostname README's generated block (and nothing else -- every other
/// caller of [`for_url`] passes a live, request-time base) is rendered
/// against: a static file has no running server to read `MCPHOST_PUBLIC_URL`
/// from, so it is baked in at `mcphost gen-docs` time against this host's
/// own production hostname, same as every other literal `https://mcphost.dev`
/// already committed throughout this README.
pub const CANONICAL_PUBLIC_URL: &str = "https://mcphost.dev";

/// AC1's four forms, plus the `mcp_url` they were all built from (so a
/// caller -- `/connect`'s page, `host_quickstart`'s response -- can show
/// the plain URL too, per the PRD's migration note: "clients without
/// deep-link support still get the plain URL on the page").
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Links {
    pub mcp_url: String,
    pub cursor: String,
    pub vscode: String,
    pub claude_code_command: String,
    pub claude_ai_steps: Vec<String>,
}

fn percent_encode(s: &str) -> String {
    utf8_percent_encode(s, NON_ALPHANUMERIC).to_string()
}

/// AC1: builds every form from `base` (e.g. `MCPHOST_PUBLIC_URL` for the
/// anonymous case, or a tenant's own `/u/<secret>` prefix for the personal
/// case) -- trailing slash tolerated the same way every other `{base}/mcp`
/// endpoint in this crate already is (see e.g. `control::key_rotate`'s own
/// `trim_end_matches('/')`).
pub fn for_url(base: &str) -> Links {
    let mcp_url = format!("{}/mcp", base.trim_end_matches('/'));

    let cursor_config = json!({"url": mcp_url}).to_string();
    let cursor_config_b64 = base64::engine::general_purpose::STANDARD.encode(&cursor_config);
    let cursor = format!(
        "cursor://anysphere.cursor-deeplink/mcp/install?name={}&config={}",
        percent_encode(CLIENT_NAME),
        percent_encode(&cursor_config_b64),
    );

    let vscode_config = json!({"name": CLIENT_NAME, "type": "http", "url": mcp_url}).to_string();
    let vscode = format!("vscode:mcp/install?{}", percent_encode(&vscode_config));

    let claude_code_command = format!("claude mcp add --transport http {CLIENT_NAME} {mcp_url}");

    let claude_ai_steps = vec![
        "Open Settings, then Connectors, then Add custom connector.".to_string(),
        format!("Name: {CLIENT_NAME}"),
        format!("Remote MCP server URL: {mcp_url}"),
        "Save, then enable the connector in a chat to connect.".to_string(),
    ];

    Links { mcp_url, cursor, vscode, claude_code_command, claude_ai_steps }
}

// ---- README generated block (P0 requirement 4, AC4) -----------------------

/// README's own `<!-- install-links:start --> ... <!-- install-links:end -->`
/// marker pair -- same splice convention `help::SUPPORT_SECTION_START`/
/// `_END` already uses for the support line, scoped to this PRD's own
/// section instead.
pub const INSTALL_LINKS_SECTION_START: &str = "<!-- install-links:start -->";
pub const INSTALL_LINKS_SECTION_END: &str = "<!-- install-links:end -->";

/// The markdown rendered between [`INSTALL_LINKS_SECTION_START`]/`_END` --
/// the single source both [`splice_install_links_section`] (the real
/// `mcphost gen-docs` write path, via `gendocs::run`) and
/// `tests/clink_ac04_*` (the hermetic drift test) call, so the two can
/// never disagree on what "generated" means.
pub fn render_readme_section(base: &str) -> String {
    let links = for_url(base);
    format!(
        "**Claude Code**\n\n\
         ```\n\
         {claude_code_command}\n\
         ```\n\n\
         **Cursor** -- [Add to Cursor]({cursor}), or add `{mcp_url}` to `mcp.json` directly.\n\n\
         **VS Code** -- [Add to VS Code]({vscode}), or add `{mcp_url}` to your MCP config directly.\n\n\
         **Claude.ai**\n\n\
         1. {step1}\n\
         2. {step2}\n\
         3. {step3}\n\
         4. {step4}",
        claude_code_command = links.claude_code_command,
        cursor = links.cursor,
        vscode = links.vscode,
        mcp_url = links.mcp_url,
        step1 = links.claude_ai_steps[0],
        step2 = links.claude_ai_steps[1],
        step3 = links.claude_ai_steps[2],
        step4 = links.claude_ai_steps[3],
    )
}

/// Replace every section bracketed by [`INSTALL_LINKS_SECTION_START`]/
/// [`INSTALL_LINKS_SECTION_END`] in `content` with [`render_readme_section`]'s
/// current rendering of `base`, leaving everything outside the markers
/// untouched -- same shape as `help::splice_support_section`. A doc with no
/// marker pair at all is returned unchanged.
pub fn splice_install_links_section(content: &str, base: &str) -> String {
    let section =
        format!("{INSTALL_LINKS_SECTION_START}\n{}\n{INSTALL_LINKS_SECTION_END}", render_readme_section(base));
    let mut out = String::new();
    let mut rest = content;
    loop {
        match (rest.find(INSTALL_LINKS_SECTION_START), rest.find(INSTALL_LINKS_SECTION_END)) {
            (Some(start), Some(end_marker_pos)) if end_marker_pos > start => {
                let end = end_marker_pos + INSTALL_LINKS_SECTION_END.len();
                out.push_str(&rest[..start]);
                out.push_str(&section);
                rest = &rest[end..];
            }
            _ => break,
        }
    }
    out.push_str(rest);
    out
}
