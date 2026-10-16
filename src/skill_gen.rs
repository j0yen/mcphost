//! PRD-mcphost-plugin-skill-generated-from-tools-list: the plugin skill's
//! reference sections are rendered from the host, never typed. This module
//! is the pure, synchronous half -- rendering, splicing and diffing --
//! shared by the `mcphost skill-gen` subcommand (`main.rs`) and the
//! `skgn_*` tests, so neither needs a running server or a DB.
//!
//! Each section lives between `<!-- mcphost-<section>:start -->` and
//! `:end` markers (the shape of the `mcphost-sandbox-api` block) in both
//! [`SKILL_PATH`] and [`WWW_SKILL_PATH`]; everything outside the markers is
//! hand-written narrative and is never touched.

use std::collections::BTreeMap;

use crate::handler::host_tool_descriptors;
use crate::kinds::KindRegistry;

pub const SKILL_PATH: &str = "plugin/skills/mcphost/SKILL.md";
pub const WWW_SKILL_PATH: &str = "www/skill.md";

/// One generated section of the skill.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Section {
    Verbs,
    Errors,
    Limits,
    Devloop,
}

impl Section {
    pub const ALL: &'static [Section] = &[Section::Verbs, Section::Limits, Section::Errors, Section::Devloop];

    /// The sections that render without the binary's clap tree, so a test
    /// can render them in memory.
    pub const LIB: &'static [Section] = &[Section::Verbs, Section::Limits, Section::Errors];

    pub fn name(self) -> &'static str {
        match self {
            Section::Verbs => "verbs",
            Section::Errors => "errors",
            Section::Limits => "limits",
            Section::Devloop => "devloop",
        }
    }

    pub fn parse(name: &str) -> Option<Section> {
        Self::ALL.iter().copied().find(|s| s.name() == name)
    }

    pub fn start_marker(self) -> String {
        format!("<!-- mcphost-{}:start -->", self.name())
    }

    pub fn end_marker(self) -> String {
        format!("<!-- mcphost-{}:end -->", self.name())
    }
}

/// One host verb as the skill states it: its wire name and its description
/// (any length -- [`first_sentence`] shortens it at render time).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerbRow {
    pub name: String,
    pub description: String,
    /// `_meta.alias_of`: the canonical this name is a clone of. A dotted
    /// clone is a deprecated alias and renders as one; an undotted clone is
    /// the flattened `a_b_c` form MCP clients derive and is left out.
    pub alias_of: Option<String>,
}

/// The verbs `tools/list` serves a tenant, read off the registry's own
/// descriptor table -- the same `host_tools(kinds, true)` call `llms-txt`
/// and the contract dump read.
pub fn registry_verbs(kinds: &KindRegistry) -> Vec<VerbRow> {
    host_tool_descriptors(kinds)
        .into_iter()
        .map(|t| VerbRow {
            name: t.name.to_string(),
            description: t.description.as_deref().unwrap_or("").to_string(),
            alias_of: t
                .meta
                .as_ref()
                .and_then(|m| m.0.get("alias_of"))
                .and_then(serde_json::Value::as_str)
                .map(str::to_string),
        })
        .collect()
}

/// Verbs read from a `tools/list` JSON-RPC response (or its bare `result`).
pub fn verbs_from_tools_list(response: &serde_json::Value) -> Vec<VerbRow> {
    let tools = response
        .get("result")
        .unwrap_or(response)
        .get("tools")
        .and_then(serde_json::Value::as_array)
        .cloned()
        .unwrap_or_default();
    tools
        .iter()
        .filter_map(|t| {
            Some(VerbRow {
                name: t.get("name")?.as_str()?.to_string(),
                description: t.get("description").and_then(|d| d.as_str()).unwrap_or("").to_string(),
                alias_of: t
                    .get("_meta")
                    .and_then(|m| m.get("alias_of"))
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string),
            })
        })
        .collect()
}

/// `description` up to and including its first sentence, whitespace
/// collapsed to single spaces. A sentence ends at a `.`, `!` or `?` followed
/// by whitespace or the end of the text.
pub fn first_sentence(description: &str) -> String {
    let flat = description.split_whitespace().collect::<Vec<_>>().join(" ");
    let bytes = flat.as_bytes();
    for (i, b) in bytes.iter().enumerate() {
        if matches!(b, b'.' | b'!' | b'?') && bytes.get(i + 1).is_none_or(|n| *n == b' ') {
            return flat[..=i].to_string();
        }
    }
    flat
}

/// The dotted family of a verb: everything before its last dot
/// (`host.tool.test` -> `host.tool`, `host.whoami` -> `host`), or `None` for
/// an undotted name (`signup`).
fn family_of(name: &str) -> Option<&str> {
    name.rsplit_once('.').map(|(family, _)| family)
}

/// The `verbs` section body: one heading per dotted family, one bullet per
/// verb, sorted; a registered alias renders as `(alias of ...)`.
pub fn render_verbs(rows: &[VerbRow]) -> String {
    let mut families: BTreeMap<String, Vec<&VerbRow>> = BTreeMap::new();
    for row in rows {
        if row.alias_of.is_some() && !row.name.contains('.') {
            continue;
        }
        let label = match family_of(&row.name) {
            Some(family) => format!("{family}.*"),
            None => "(top level)".to_string(),
        };
        families.entry(label).or_default().push(row);
    }
    let mut out = String::new();
    out.push_str(&Section::Verbs.start_marker());
    out.push_str(
        "\nGenerated by `mcphost skill-gen` from the host's `tools/list`; do not hand-edit.\n",
    );
    for (label, mut verbs) in families {
        verbs.sort_by(|a, b| a.name.cmp(&b.name));
        verbs.dedup_by(|a, b| a.name == b.name);
        out.push_str(&format!("\n#### `{label}`\n\n"));
        for v in verbs {
            match v.alias_of.as_deref() {
                Some(canonical) => {
                    out.push_str(&format!("- `{}` -- (alias of `{canonical}`)\n", v.name));
                }
                None => {
                    out.push_str(&format!("- `{}` -- {}\n", v.name, first_sentence(&v.description)));
                }
            }
        }
    }
    out.push_str(&Section::Verbs.end_marker());
    out
}

/// The `errors` section body: one table row per [`crate::errors::error_codes`]
/// entry with its [`crate::errors::remedy`].
pub fn render_errors() -> String {
    let mut out = String::new();
    out.push_str(&Section::Errors.start_marker());
    out.push_str("\nGenerated by `mcphost skill-gen` from `AppError::code` and `errors::remedy`; do not hand-edit.\n\n");
    out.push_str("| code | what to do |\n|---|---|\n");
    for code in crate::errors::error_codes() {
        out.push_str(&format!("| `{code}` | {} |\n", crate::errors::remedy(code)));
    }
    out.push_str(&Section::Errors.end_marker());
    out
}

/// The `limits` section body: one row per [`crate::plans::Plan::ceilings_flat`]
/// key of the `free` plan (names verbatim from `ceilings_json`, `budget`
/// fields dotted), each with its `free` and `pro` value, then the plan the
/// public-network gate needs. Every number comes from the catalog.
pub fn render_limits(catalog: &crate::plans::PlanCatalog) -> String {
    let flat = |name: &str| -> BTreeMap<String, i64> {
        catalog.get(name).map(|p| p.ceilings_flat().into_iter().collect()).unwrap_or_default()
    };
    let (free, pro) = (flat("free"), flat("pro"));
    let mut out = String::new();
    out.push_str(&Section::Limits.start_marker());
    out.push_str(
        "\nGenerated by `mcphost skill-gen` from `plan.ceilings_json()`; do not hand-edit. \
         `host.quickstart` returns your plan's own values under `limits.plan`.\n\n",
    );
    out.push_str("| limit | free | pro |\n|---|---|---|\n");
    for (key, value) in &free {
        let pro_value = pro.get(key).map_or("-".to_string(), i64::to_string);
        out.push_str(&format!("| `{key}` | {value} | {pro_value} |\n"));
    }
    out.push_str(&format!(
        "\nGates: `network: public` needs the `{}` plan.\n",
        crate::network_policy::EGRESS_PLAN
    ));
    out.push_str(&Section::Limits.end_marker());
    out
}

/// The command groups the `devloop` section lists, when the binary has them.
pub const DEVLOOP_COMMANDS: &[&str] = &["tool", "dev", "push", "repo"];

/// The `devloop` section body, rendered from the CLI's own clap definitions:
/// each of [`DEVLOOP_COMMANDS`] the binary has, with its visible
/// subcommands. A verb absent from the binary is absent from the section.
pub fn render_devloop(cli: &clap::Command) -> String {
    let mut out = String::new();
    out.push_str(&Section::Devloop.start_marker());
    out.push_str("\nGenerated by `mcphost skill-gen` from the CLI's clap definitions; do not hand-edit.\n\n");
    let present: Vec<&clap::Command> = DEVLOOP_COMMANDS
        .iter()
        .filter_map(|name| cli.get_subcommands().find(|c| c.get_name() == *name && !c.is_hide_set()))
        .collect();
    if present.is_empty() {
        out.push_str("no local verbs in this build\n");
    }
    let about = |c: &clap::Command| {
        c.get_about().map_or(String::new(), |a| format!(" -- {}", first_sentence(&a.to_string())))
    };
    for group in present {
        out.push_str(&format!("- `{} {}`{}\n", cli.get_name(), group.get_name(), about(group)));
        for sub in group.get_subcommands().filter(|c| !c.is_hide_set()) {
            out.push_str(&format!(
                "  - `{} {} {}`{}\n",
                cli.get_name(),
                group.get_name(),
                sub.get_name(),
                about(sub)
            ));
        }
    }
    out.push_str(&Section::Devloop.end_marker());
    out
}

/// What each section renders from. `verbs` is a parameter so the same file
/// can be rendered from the registry or from a live `tools/list`.
pub struct Sources {
    pub verbs: Vec<VerbRow>,
    pub plans: crate::plans::PlanCatalog,
    /// The binary's clap tree (`devloop`). Empty until the caller that owns
    /// the `Cli` type sets it; see [`Section::LIB`].
    pub cli: clap::Command,
}

impl Sources {
    pub fn from_registry(kinds: &KindRegistry) -> Sources {
        Sources { verbs: registry_verbs(kinds), plans: crate::plans::PlanCatalog::default_catalog(),
            cli: clap::Command::new("mcphost"),
        }
    }
}

pub fn render_section(section: Section, sources: &Sources) -> String {
    match section {
        Section::Verbs => render_verbs(&sources.verbs),
        Section::Errors => render_errors(),
        Section::Limits => render_limits(&sources.plans),
        Section::Devloop => render_devloop(&sources.cli),
    }
}

/// Replace `section`'s marked block in `content` with `rendered`. `Err`
/// when the file does not carry exactly one start and one end marker.
pub fn splice(content: &str, section: Section, rendered: &str) -> Result<String, String> {
    let (start, end) = (section.start_marker(), section.end_marker());
    if content.matches(&start).count() != 1 || content.matches(&end).count() != 1 {
        return Err(format!("must carry exactly one {start} / {end} pair"));
    }
    let s = content.find(&start).unwrap_or(0);
    let e = content.find(&end).unwrap_or(0) + end.len();
    if e <= s {
        return Err(format!("{end} precedes {start}"));
    }
    Ok(format!("{}{}{}", &content[..s], rendered, &content[e..]))
}

/// `content` with every one of `sections` re-rendered.
pub fn render_file(content: &str, sections: &[Section], sources: &Sources) -> Result<String, String> {
    let mut out = content.to_string();
    for section in sections {
        out = splice(&out, *section, &render_section(*section, sources))?;
    }
    Ok(out)
}

/// The 1-based line of the first difference between `actual` and
/// `expected`, or `None` when they are byte-equal.
pub fn first_diff_line(actual: &str, expected: &str) -> Option<usize> {
    if actual == expected {
        return None;
    }
    let mut a = actual.lines();
    let mut e = expected.lines();
    let mut n = 0;
    loop {
        n += 1;
        match (a.next(), e.next()) {
            (Some(x), Some(y)) if x == y => continue,
            (None, None) => return Some(n.max(1)),
            _ => return Some(n),
        }
    }
}

/// Fetch every page of `tools/list` from the MCP endpoint at `url` (the
/// stateless request shape `mcphost docs-examples` speaks), presenting
/// `bearer` when given. The reply may be JSON or a one-event SSE stream.
pub async fn fetch_tools_list(url: &str, bearer: Option<&str>) -> Result<Vec<VerbRow>, String> {
    let http = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| e.to_string())?;
    let mut rows = Vec::new();
    let mut cursor: Option<String> = None;
    for id in 1..=100u64 {
        let mut params = serde_json::json!({
            "_meta": {
                "io.modelcontextprotocol/protocolVersion": "2026-07-28",
                "io.modelcontextprotocol/clientCapabilities": {},
                "io.modelcontextprotocol/clientInfo": {"name": "mcphost-skill-gen", "version": env!("CARGO_PKG_VERSION")},
            }
        });
        if let Some(c) = &cursor {
            params["cursor"] = serde_json::json!(c);
        }
        let body = serde_json::json!({"jsonrpc": "2.0", "id": id, "method": "tools/list", "params": params});
        let mut req = http
            .post(url)
            .header("Content-Type", "application/json")
            .header("Accept", "application/json, text/event-stream")
            .header("MCP-Protocol-Version", "2026-07-28")
            .header("Mcp-Method", "tools/list");
        if let Some(key) = bearer {
            req = req.header("Authorization", format!("Bearer {key}"));
        }
        let resp = req.json(&body).send().await.map_err(|e| format!("tools/list request to {url}: {e}"))?;
        let is_sse = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|c| c.starts_with("text/event-stream"));
        let status = resp.status();
        let text = resp.text().await.map_err(|e| format!("tools/list body from {url}: {e}"))?;
        let message: serde_json::Value = if is_sse {
            text.lines()
                .filter_map(|l| l.strip_prefix("data:"))
                .filter_map(|d| serde_json::from_str(d.trim()).ok())
                .next_back()
                .ok_or_else(|| format!("{url}: empty event stream ({status})"))?
        } else {
            serde_json::from_str(&text).map_err(|e| format!("{url}: HTTP {status}, body is not JSON: {e}"))?
        };
        if let Some(error) = message.get("error") {
            return Err(format!("{url}: tools/list refused: {error}"));
        }
        rows.extend(verbs_from_tools_list(&message));
        cursor = message
            .get("result")
            .and_then(|r| r.get("nextCursor"))
            .and_then(serde_json::Value::as_str)
            .map(str::to_string);
        if cursor.is_none() {
            return Ok(rows);
        }
    }
    Err(format!("{url}: tools/list paged more than 100 times"))
}
