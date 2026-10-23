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

/// The fifteen surface ids, in table order. `www/install-surfaces.json`
/// is generated from this list ([`render_surfaces_json`]).
pub const SURFACE_IDS: [&str; 15] = [
    "claude_code",
    "cursor",
    "vscode",
    "copilot_vscode",
    "claude_ai",
    "chatgpt",
    "codex_cli",
    "gemini_cli",
    "opencode",
    "amp",
    "goose",
    "warp",
    "windsurf",
    "cline",
    "sdk",
];

/// The surface set the funnel reports by (PRD-mcphost-funnel-by-surface-
/// nightly-probe R1): the same slice the README install section and the
/// `host.quickstart` client list are rendered from.
pub const SURFACES: &[&str] = &SURFACE_IDS;

/// Extra lower-cased `clientInfo.name` prefixes the funnel maps to a
/// surface id beyond [`CLIENT_INFO_NAMES`] (the install doc's own table):
/// clients that announce themselves by a name the install doc does not need
/// to list. Every id here must be in [`SURFACES`].
pub const SURFACE_EXTRA_PREFIXES: &[(&str, &str)] = &[("chatgpt", "chatgpt"), ("openai-mcp", "chatgpt")];

/// Folds a client name to its alphanumerics, lower-cased, so
/// `Claude Code 2.0`, `claude-code` and `claude_code` compare equal.
fn fold_client_name(name: &str) -> String {
    name.chars().filter(|c| c.is_ascii_alphanumeric()).map(|c| c.to_ascii_lowercase()).collect()
}

/// R1: maps a stored `tenants.client_name` to one id from [`SURFACES`];
/// unknown or `NULL` is `other`. Prefix match, case- and
/// punctuation-insensitive, over [`CLIENT_INFO_NAMES`] then
/// [`SURFACE_EXTRA_PREFIXES`].
pub fn surface_id(client_name: Option<&str>) -> &'static str {
    let Some(name) = client_name else { return "other" };
    let folded = fold_client_name(name);
    CLIENT_INFO_NAMES
        .iter()
        .chain(SURFACE_EXTRA_PREFIXES.iter())
        .find(|(prefix, _)| folded.starts_with(&fold_client_name(prefix)))
        .map(|(_, id)| *id)
        .unwrap_or("other")
}

/// The full install table built from one base URL, plus the `mcp_url` it
/// was built from and the original four forms under their original field
/// names: a thin compatibility view (the PRD's technical considerations),
/// so PRD-mcphost-client-install-links' callers and tests keep working.
/// Derefs to the `[Surface]` table.
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
/// ChatGPT row (PRD-mcphost-chatgpt-submission-pack): steps only, no deep
/// link exists; `doc_url` is OpenAI's MCP page, `checked` come from `docs/clients.toml`.
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

    // PRD-mcphost-oauth-first-entry-url requirement 6: the OAuth-only clients
    // (Claude.ai, ChatGPT) are given the OAuth-first entry URL, the one URL
    // that answers them with the 401 their connector flow needs. A personal
    // `/u/<secret>` base already carries its own credential and keeps its
    // own `/mcp` URL.
    let oauth_url = if base.contains("/u/") { mcp_url.clone() } else { crate::oauth::entry_resource_uri(base) };

    let claude_ai_steps = vec![
        "Open Settings, then Connectors, then Add custom connector.".to_string(),
        format!("Name: {CLIENT_NAME}"),
        format!("Remote MCP server URL: {oauth_url}"),
        "Save, then enable the connector in a chat to connect.".to_string(),
    ];

    let chatgpt_steps = vec![
        "Open ChatGPT, then Plugins, then Add.".to_string(),
        "Choose Create custom MCP server.".to_string(),
        format!("Name: {CLIENT_NAME}"),
        format!("MCP server URL: {oauth_url}"),
        "Choose OAuth, then Create. On the consent page press Create a new workspace.".to_string(),
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
    // Copilot Chat in VS Code reads the same `.vscode/mcp.json` as VS Code
    // itself (top-level `servers`, not `mcpServers`); the SDK row is the
    // remote-server object the Anthropic MCP connector takes.
    let copilot_json = serde_json::to_string_pretty(
        &json!({"servers": {CLIENT_NAME: {"type": "http", "url": mcp_url}}}),
    )
    .unwrap_or_default();
    let sdk_json =
        serde_json::to_string_pretty(&json!({"type": "url", "url": mcp_url, "name": CLIENT_NAME})).unwrap_or_default();
    let json_block = |extra: serde_json::Value| {
        let mut server = json!({"url": mcp_url});
        if let (Some(obj), Some(extra)) = (server.as_object_mut(), extra.as_object()) {
            obj.extend(extra.clone());
        }
        serde_json::to_string_pretty(&json!({"mcpServers": {CLIENT_NAME: server}})).unwrap_or_default()
    };

    // `doc_url`/`checked` come from `docs/clients.toml` (keyed by label), the
    // one place a client's docs link lives -- see `crate::clients`.
    let row = |id, label, kind, artefact: String| {
        let (doc_url, checked) = crate::clients::docs_for(label);
        Surface { id, label, kind, artefact, doc_url, checked }
    };
    let surfaces = vec![
        row("claude_code", "Claude Code", Kind::Command, claude_code_command.clone()),
        row("cursor", "Cursor", Kind::Deeplink, cursor.clone()),
        row("vscode", "VS Code", Kind::Deeplink, vscode.clone()),
        row("copilot_vscode", "GitHub Copilot in VS Code", Kind::Json, copilot_json),
        row("claude_ai", "Claude.ai", Kind::Steps, numbered(&claude_ai_steps)),
        row("chatgpt", "ChatGPT", Kind::Steps, numbered(&chatgpt_steps)),
        row("codex_cli", "Codex CLI", Kind::Command, format!("codex mcp add {CLIENT_NAME} --url {mcp_url}")),
        row("gemini_cli", "Gemini CLI", Kind::Command, format!("gemini mcp add --transport http {CLIENT_NAME} {mcp_url}")),
        row("opencode", "OpenCode", Kind::Command, format!("opencode mcp add {CLIENT_NAME} --url {mcp_url}")),
        row("amp", "Amp", Kind::Command, format!("amp mcp add {CLIENT_NAME} {mcp_url}")),
        row("goose", "Goose", Kind::Steps, numbered(&goose_steps)),
        row("warp", "Warp", Kind::Steps, numbered(&warp_steps)),
        row("windsurf", "Windsurf", Kind::Json, json_block(json!({}))),
        row("cline", "Cline / Roo", Kind::Json, json_block(json!({"type": "streamableHttp"}))),
        row("sdk", "SDKs", Kind::Json, sdk_json),
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

/// `User-Agent` substrings (lower-cased) a client sends on `/mcp` that name
/// its surface, for callers that send no usable `clientInfo`
/// (PRD-mcphost-quickstart-by-client R1). Every id here must be in
/// [`SURFACE_IDS`] (asserted by `tests/qsbc_ac01_*`).
pub const USER_AGENT_NAMES: &[(&str, &str)] = &[
    ("claude-code", "claude_code"),
    ("cursor", "cursor"),
    ("codex", "codex_cli"),
    ("gemini", "gemini_cli"),
    ("opencode", "opencode"),
    ("vscode", "vscode"),
    ("windsurf", "windsurf"),
];

/// One row per surface id: `(surface id, caveats, next tool)`
/// (PRD-mcphost-quickstart-by-client R1). The only prose a surface carries
/// beyond its install artefact; `tests/qsbc_ac01_*` asserts the ids equal
/// [`SURFACE_IDS`] exactly, so a new surface without a row fails the build.
pub const SURFACE_CAVEATS: &[(&str, &[&str], &str)] = &[
    ("claude_code", &["Run the command in a shell; restart Claude Code if /mcp does not list mcphost"], "host.whoami"),
    ("cursor", &["Cursor opens the deep link in the editor; the first request on /mcp creates your tenant"], "host.whoami"),
    ("vscode", &["VS Code opens the deep link in the editor; confirm the install prompt it shows"], "host.whoami"),
    (
        "copilot_vscode",
        &["Merge the JSON block into .vscode/mcp.json (top-level `servers`); Copilot Chat in agent mode then lists mcphost's tools"],
        "host.whoami",
    ),
    (
        "claude_ai",
        &["Claude.ai connects over OAuth only: approve the consent page, then call host.whoami"],
        "host.whoami",
    ),
    (
        "chatgpt",
        &[
            "ChatGPT connects over OAuth only: on the consent page press \"Create a new workspace\", then call host.whoami",
        ],
        "host.whoami",
    ),
    ("codex_cli", &["Run the command in a shell; Codex reads the server list at startup, so restart it"], "host.whoami"),
    ("gemini_cli", &["Run the command in a shell; Gemini CLI reads the server list at startup, so restart it"], "host.whoami"),
    ("opencode", &["Run the command in a shell; OpenCode reads the server list at startup, so restart it"], "host.whoami"),
    ("amp", &["Run the command in a shell; reload Amp so it lists mcphost's tools"], "host.whoami"),
    ("goose", &["Goose adds the server from a session via /extension; there is no command to run"], "host.whoami"),
    ("warp", &["Warp adds the server from its agent settings; there is no command to run"], "host.whoami"),
    ("windsurf", &["Merge the JSON block into mcp_config.json, then refresh Windsurf's MCP list"], "host.whoami"),
    (
        "cline",
        &["Merge the JSON block into cline_mcp_settings.json; the streamableHttp type is required"],
        "host.whoami",
    ),
    (
        "sdk",
        &["From code, pass the endpoint URL to your MCP client; there is nothing to install and the first request creates your tenant"],
        "host.whoami",
    ),
];

/// R2: the surface a caller is running in, as `(surface id, from)` where
/// `from` is `"clientInfo"` or `"user_agent"`. `clientInfo.name` wins;
/// matching is case-insensitive substring over [`CLIENT_INFO_NAMES`] (then
/// [`SURFACE_EXTRA_PREFIXES`], which names ChatGPT), then
/// [`USER_AGENT_NAMES`] over the `User-Agent`.
pub fn detect(client_name: Option<&str>, user_agent: Option<&str>) -> Option<(&'static str, &'static str)> {
    let find = |text: &str, tables: &[&[(&'static str, &'static str)]]| {
        let text = text.to_ascii_lowercase();
        tables
            .iter()
            .flat_map(|t| t.iter())
            .find(|(needle, _)| text.contains(*needle))
            .map(|(_, id)| *id)
    };
    if let Some(id) = client_name.and_then(|n| find(n, &[CLIENT_INFO_NAMES, SURFACE_EXTRA_PREFIXES])) {
        return Some((id, "clientInfo"));
    }
    user_agent.and_then(|ua| find(ua, &[USER_AGENT_NAMES])).map(|id| (id, "user_agent"))
}

/// R3/R5: the `client` block `host.quickstart` and
/// `GET /connect/go/{id}?format=json` both return, rendered for `base`.
/// `detected` is `None` for an unknown caller: every field but `all` is
/// then null (`caveats` empty).
pub fn client_block(base: &str, detected: Option<(&str, &str)>) -> serde_json::Value {
    let all = install_doc_url();
    let Some((id, from)) = detected else {
        return json!({
            "detected": null, "from": null, "install": null, "snippet": null,
            "caveats": [], "next": null, "all": all,
        });
    };
    let surface = surface_for(base, id);
    let row = SURFACE_CAVEATS.iter().find(|(row_id, _, _)| *row_id == id);
    json!({
        "detected": id,
        "from": from,
        "snippet": surface.as_ref().map(|s| s.artefact.clone()),
        "install": surface,
        "caveats": row.map(|(_, caveats, _)| caveats.to_vec()).unwrap_or_default(),
        "next": row.map(|(_, _, tool)| json!({
            "tool": tool,
            "why": "First call on this connection: read-only, and it creates your tenant.",
        })),
        "all": all,
    })
}

/// `www/install-surfaces.json`: [`SURFACE_IDS`] as `{"surfaces": [...]}`, so
/// the walker that audits the install doc reads the id list from the binary
/// instead of keeping its own copy. Generated by `mcphost gen-docs`.
pub const SURFACES_JSON_PATH: &str = "www/install-surfaces.json";

pub fn render_surfaces_json() -> String {
    let mut out = serde_json::to_string_pretty(&json!({"surfaces": SURFACE_IDS})).unwrap_or_default();
    out.push('\n');
    out
}

/// One [`doc_extra`] entry: prose line, optional extra fenced block, second docs link.
type DocExtra = (&'static str, Option<(&'static str, String)>, &'static str);

/// Install-doc sections that say more than their [`Kind`] default: `(id,
/// prose line, extra fenced block as (language, body) or None, second docs
/// link)`. A surface with no entry renders the plain Kind form.
fn doc_extra(id: &str, mcp_url: &str) -> Option<DocExtra> {
    match id {
        "copilot_vscode" => Some((
            "Copilot Chat is the same VS Code client: add mcphost to `.vscode/mcp.json` (or use the VS Code install link above).",
            None,
            "https://docs.github.com/copilot/customizing-copilot/using-model-context-protocol/extending-copilot-chat-with-mcp",
        )),
        "sdk" => Some((
            "From code, point an MCP client at the endpoint; the Anthropic MCP connector and the OpenAI Agents SDK both take a URL.",
            Some(("python", format!("MCPServerStreamableHttp(params={{\"url\": \"{mcp_url}\"}})"))),
            "https://openai.github.io/openai-agents-python/mcp/",
        )),
        _ => None,
    }
}

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
        let extra = doc_extra(s.id, &links.mcp_url);
        let how = extra.as_ref().map_or(how, |(prose, _, _)| *prose);
        out.push_str(&format!(
            "## {label} (`{id}`)\n\n{how}\n\n```{fence}\n{artefact}\n```\n\n",
            label = s.label,
            id = s.id,
            artefact = s.artefact,
        ));
        if let Some((_, Some((lang, body)), _)) = &extra {
            out.push_str(&format!("```{lang}\n{body}\n```\n\n"));
        }
        out.push_str(&format!("Docs: <{}> (checked {})", s.doc_url, s.checked));
        if let Some((_, _, more)) = &extra {
            out.push_str(&format!(" and <{more}>"));
        }
        out.push_str("\n\n");
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
