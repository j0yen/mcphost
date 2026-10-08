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

/// How a surface's [`Surface::artefact`] is meant to be used: `command` is
/// one line to paste into a shell, `deeplink` is a URL the client's own
/// handler opens, `json` is a complete config block to paste into the
/// client's config file, `steps` is numbered instructions for a client
/// with no machine-readable install path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Command,
    Deeplink,
    Json,
    Steps,
}

impl Kind {
    /// Display order of the `/connect` page's per-kind headings.
    pub const ALL: [Kind; 4] = [Kind::Command, Kind::Deeplink, Kind::Json, Kind::Steps];

    pub fn heading(self) -> &'static str {
        match self {
            Kind::Command => "Run a command",
            Kind::Deeplink => "One-click install",
            Kind::Json => "Paste a JSON config",
            Kind::Steps => "Follow the steps",
        }
    }
}

/// One row of the install table (PRD-mcphost-install-links-more-clients
/// requirement 1): everything `/connect`, `/connect/go/{id}`,
/// `host.quickstart.install_links`, and the README/llms.txt block render.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Surface {
    pub id: &'static str,
    pub label: &'static str,
    pub kind: Kind,
    pub artefact: String,
    pub doc_url: &'static str,
    pub checked: &'static str,
}

/// The twelve surface ids, in table order.
pub const SURFACE_IDS: [&str; 12] = [
    "claude_code",
    "cursor",
    "vscode",
    "claude_ai",
    "codex_cli",
    "gemini_cli",
    "opencode",
    "amp",
    "goose",
    "warp",
    "windsurf",
    "cline",
];

/// Date every row's `doc_url` was last read against its artefact.
const CHECKED: &str = "2026-10-07";

/// The full install table built from one base URL, plus the `mcp_url` it
/// was built from and the original four forms under their original field
/// names: a thin compatibility view (the PRD's technical considerations),
/// so PRD-mcphost-client-install-links' callers and tests keep working.
/// Derefs to the 12-row `[Surface]` table.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Links {
    pub mcp_url: String,
    pub cursor: String,
    pub vscode: String,
    pub claude_code_command: String,
    pub claude_ai_steps: Vec<String>,
    pub surfaces: Vec<Surface>,
}

impl std::ops::Deref for Links {
    type Target = [Surface];
    fn deref(&self) -> &[Surface] {
        &self.surfaces
    }
}

impl<'a> IntoIterator for &'a Links {
    type Item = &'a Surface;
    type IntoIter = std::slice::Iter<'a, Surface>;
    fn into_iter(self) -> Self::IntoIter {
        self.surfaces.iter()
    }
}

fn percent_encode(s: &str) -> String {
    utf8_percent_encode(s, NON_ALPHANUMERIC).to_string()
}

fn numbered(steps: &[String]) -> String {
    steps.iter().enumerate().map(|(i, s)| format!("{}. {s}", i + 1)).collect::<Vec<_>>().join("\n")
}

/// AC1: builds every surface from `base` (e.g. `MCPHOST_PUBLIC_URL` for the
/// anonymous case, or a tenant's own `/u/<secret>` prefix for the personal
/// case) -- trailing slash tolerated the same way every other `{base}/mcp`
/// endpoint in this crate already is (see e.g. `control::key_rotate`'s own
/// `trim_end_matches('/')`).
///
/// Rows added by PRD-mcphost-install-links-more-clients (each `doc_url`
/// read 2026-10-07; flag spellings are the ones that page documents):
/// Codex CLI `codex mcp add <name> --url <url>`; Gemini CLI
/// `gemini mcp add --transport http <name> <url>`; OpenCode
/// `opencode mcp add <name> --url <url>`; Amp `amp mcp add <name> <url>`;
/// Goose `/extension`; Warp `/agent-add-mcp`; Windsurf `mcp_config.json`;
/// Cline `cline_mcp_settings.json`.
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

    let goose_steps = vec![
        "In a Goose session, type /extension.".to_string(),
        "Choose Add Remote Extension (Streamable HTTP).".to_string(),
        format!("Name: {CLIENT_NAME}"),
        format!("Endpoint URL: {mcp_url}"),
    ];
    let warp_steps = vec![
        "In a Warp agent session, type /agent-add-mcp.".to_string(),
        format!("Paste this server config: {}", json!({CLIENT_NAME: {"url": mcp_url}})),
        "Save; Warp starts the server and lists its tools.".to_string(),
    ];
    let json_block = |extra: serde_json::Value| {
        let mut server = json!({"url": mcp_url});
        if let (Some(obj), Some(extra)) = (server.as_object_mut(), extra.as_object()) {
            obj.extend(extra.clone());
        }
        serde_json::to_string_pretty(&json!({"mcpServers": {CLIENT_NAME: server}})).unwrap_or_default()
    };

    let row = |id, label, kind, artefact: String, doc_url| Surface {
        id,
        label,
        kind,
        artefact,
        doc_url,
        checked: CHECKED,
    };
    let surfaces = vec![
        row("claude_code", "Claude Code", Kind::Command, claude_code_command.clone(), "https://code.claude.com/docs/en/mcp"),
        row("cursor", "Cursor", Kind::Deeplink, cursor.clone(), "https://docs.cursor.com/en/tools/mcp"),
        row("vscode", "VS Code", Kind::Deeplink, vscode.clone(), "https://code.visualstudio.com/api/extension-guides/ai/mcp"),
        row("claude_ai", "Claude.ai", Kind::Steps, numbered(&claude_ai_steps), "https://support.claude.com/en/articles/11175166-get-started-with-custom-connectors-using-remote-mcp"),
        row("codex_cli", "Codex CLI", Kind::Command, format!("codex mcp add {CLIENT_NAME} --url {mcp_url}"), "https://developers.openai.com/codex/mcp"),
        row("gemini_cli", "Gemini CLI", Kind::Command, format!("gemini mcp add --transport http {CLIENT_NAME} {mcp_url}"), "https://geminicli.com/docs/tools/mcp-server/"),
        row("opencode", "OpenCode", Kind::Command, format!("opencode mcp add {CLIENT_NAME} --url {mcp_url}"), "https://opencode.ai/docs/mcp-servers/"),
        row("amp", "Amp", Kind::Command, format!("amp mcp add {CLIENT_NAME} {mcp_url}"), "https://ampcode.com/manual#mcp"),
        row("goose", "Goose", Kind::Steps, numbered(&goose_steps), "https://block.github.io/goose/docs/getting-started/using-extensions/"),
        row("warp", "Warp", Kind::Steps, numbered(&warp_steps), "https://docs.warp.dev/agent-platform/capabilities/mcp"),
        row("windsurf", "Windsurf", Kind::Json, json_block(json!({})), "https://docs.windsurf.com/windsurf/cascade/mcp"),
        row("cline", "Cline / Roo", Kind::Json, json_block(json!({"type": "streamableHttp"})), "https://docs.cline.bot/mcp/configuring-mcp-servers"),
    ];

    Links { mcp_url, cursor, vscode, claude_code_command, claude_ai_steps, surfaces }
}

/// Looks one surface up by id in the table built from `base`; `None` for
/// any id outside [`SURFACE_IDS`].
pub fn surface_for(base: &str, id: &str) -> Option<Surface> {
    for_url(base).surfaces.into_iter().find(|s| s.id == id)
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
    let mut blocks = Vec::new();
    for s in &links {
        let body = match s.kind {
            Kind::Command => format!("```\n{}\n```", s.artefact),
            Kind::Deeplink => format!(
                "[Add to {label}]({artefact}), or add `{mcp_url}` to your MCP config directly.",
                label = s.label,
                artefact = s.artefact,
                mcp_url = links.mcp_url,
            ),
            Kind::Json => format!("```json\n{}\n```", s.artefact),
            Kind::Steps => s.artefact.clone(),
        };
        blocks.push(format!(
            "**{label}**\n\n{body}\n\nDocs: <{doc_url}> (checked {checked})",
            label = s.label,
            doc_url = s.doc_url,
            checked = s.checked,
        ));
    }
    blocks.join("\n\n")
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

// ---- llms-install.md (PRD-mcphost-llms-install-doc) -------------------------

/// Route and repo-root file name of the agent-readable install document.
pub const INSTALL_DOC_FILE: &str = "llms-install.md";

/// Absolute URL of the install document on the production host -- the one
/// string `www/llms.txt`, `www/skill.md`, the README and
/// `host.quickstart.install_doc_url` all carry.
pub fn install_doc_url() -> String {
    format!("{CANONICAL_PUBLIC_URL}/{INSTALL_DOC_FILE}")
}

/// `clientInfo.name` values (lower-cased, matched as prefixes) an MCP client
/// announces in `initialize`, mapped to the install-table surface id an agent
/// running inside that client should use. Names not listed fall through to
/// the "pick your client" list in the document. Every id here must be in
/// [`SURFACE_IDS`] (asserted by `tests/mcphost-llms-install-doc_ac01_*`).
pub const CLIENT_INFO_NAMES: &[(&str, &str)] = &[
    ("claude-code", "claude_code"),
    ("claude-ai", "claude_ai"),
    ("cursor", "cursor"),
    ("visual studio code", "vscode"),
    ("vscode", "vscode"),
    ("codex", "codex_cli"),
    ("gemini-cli", "gemini_cli"),
    ("opencode", "opencode"),
    ("amp", "amp"),
    ("goose", "goose"),
    ("warp", "warp"),
    ("windsurf", "windsurf"),
    ("cline", "cline"),
    ("roo", "cline"),
    ("kilo", "cline"),
];

/// The agent-readable install document for `base`: a "which client am I"
/// table, one `## ` section per install surface (artefact in a fenced
/// block), then the "first call" section. Generated, never hand-edited:
/// the committed root `llms-install.md` and `GET /llms-install.md` are both
/// this function's output.
pub fn render_install_doc(base: &str) -> String {
    let links = for_url(base);
    let base = base.trim_end_matches('/');
    let mut out = String::new();
    out.push_str("# Install mcphost (written for agents)\n\n");
    out.push_str(&format!(
        "You are an agent asked to connect to mcphost. The server is `{}`; it needs no \
         account, key or header. Find your client below, apply its install step yourself, \
         then make the first call. Generated by `mcphost gen-docs` from the install table; \
         do not hand-edit.\n\n",
        links.mcp_url
    ));
    out.push_str("## Which client am I?\n\n");
    out.push_str(
        "Read the `clientInfo.name` you send in `initialize` (or the name of the app you run \
         in) and pick the matching section. If none matches, choose the closest client from \
         the list of sections below.\n\n",
    );
    out.push_str("| `clientInfo.name` starts with | Section |\n|---|---|\n");
    for (name, id) in CLIENT_INFO_NAMES {
        let label = links.surfaces.iter().find(|s| s.id == *id).map(|s| s.label).unwrap_or(id);
        out.push_str(&format!("| `{name}` | {label} (`{id}`) |\n"));
    }
    out.push('\n');
    for s in &links {
        let (how, fence) = match s.kind {
            Kind::Command => ("Run this command in a shell:", "sh"),
            Kind::Deeplink => ("Open this link (the client handles it), or add the endpoint to its MCP config by hand:", "text"),
            Kind::Json => ("Merge this block into the client's MCP config file:", "json"),
            Kind::Steps => ("Follow these steps:", "text"),
        };
        out.push_str(&format!(
            "## {label} (`{id}`)\n\n{how}\n\n```{fence}\n{artefact}\n```\n\nDocs: <{doc_url}> (checked {checked})\n\n",
            label = s.label,
            id = s.id,
            artefact = s.artefact,
            doc_url = s.doc_url,
            checked = s.checked,
        ));
    }
    out.push_str(&format!(
        "## First call\n\n\
         1. After installing, restart or reload the client's MCP connections if it asks you to.\n\
         2. There is no signup step: your first `host.*` call creates your tenant on this \
         connection. Call `host.whoami` -- it is read-only and cheap.\n\
         3. Read `onboarding.url` in that response and save it where you keep long-lived notes. \
         It is your tenant's own address and credential in one; reconnecting through it later is \
         how you come back as the same tenant.\n\
         4. If your human sent you an invite, connect through `{base}/i/<code>/mcp` instead of \
         `{mcp}`.\n\
         5. `host.quickstart` returns a worked example and your own personal install links.\n",
        mcp = links.mcp_url,
    ));
    out
}
