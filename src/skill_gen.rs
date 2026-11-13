//! PRD-mcphost-plugin-skill-generated-from-registry: the marked sections of
//! `plugin/skills/mcphost/SKILL.md` (verbs, limits, errors, local loop) are
//! rendered here from the registries the host itself serves, so the skill an
//! agent reads first can never name a verb, ceiling or remedy the host does
//! not have. Pure and synchronous apart from [`fetch_tool_names`]; shared by
//! the `mcphost skill-gen` subcommand and the `skgn_*` tests.

use std::collections::{BTreeMap, BTreeSet};

use crate::kinds::KindRegistry;
use crate::tool_aliases;

/// The plugin skill: the one file `skill-gen` renders into.
pub const PLUGIN_SKILL_PATH: &str = "plugin/skills/mcphost/SKILL.md";
/// `gen-docs` keeps this byte-equal to the plugin file.
pub const WWW_SKILL_PATH: &str = "www/skill.md";

/// The section names `--section` accepts, in the order they appear in the file.
pub const SECTIONS: &[&str] = &["verbs", "limits", "errors", "devloop"];

/// The CLI verbs of the local tool loop, in the order the section lists them.
pub const DEVLOOP_VERBS: &[&str] = &["tool", "dev", "push"];

pub fn start_marker(section: &str) -> String {
    format!("<!-- mcphost-{section}:start -->")
}

pub fn end_marker(section: &str) -> String {
    format!("<!-- mcphost-{section}:end -->")
}

/// Every name the host advertises, aliases and flattened forms included.
pub fn registry_tool_names(kinds: &KindRegistry) -> Vec<String> {
    let mut names = crate::handler::llms_txt_tool_names(kinds);
    let signup = crate::handler::signup_tool().name.to_string();
    if !names.contains(&signup) {
        names.push(signup);
    }
    names
}

/// `true` for a name that is a verb in its own right: not a deprecated
/// alias and not a flattened `a_b_c` clone.
fn is_canonical(name: &str) -> bool {
    if tool_aliases::resolve(name).is_some() {
        return false;
    }
    name.contains('.') || !(name.starts_with("host_") || name.starts_with("billing_"))
}

/// `host.table.create` -> (`host.table.*`, `create`); a two-segment name
/// such as `host.whoami` belongs to the `host.*` family.
fn family_of(name: &str) -> (String, String) {
    match name.rsplit_once('.') {
        Some((family, leaf)) => (format!("{family}.*"), leaf.to_string()),
        None => (name.to_string(), name.to_string()),
    }
}

/// The `verbs` section: one line per dotted family with its count and verb
/// names, the anonymous starter verbs named once, no per-verb description.
/// `names` is every advertised name; `starters` the anonymous starter set.
pub fn render_verbs(names: &[String], starters: &[String]) -> String {
    let present: BTreeSet<&str> = names.iter().map(String::as_str).collect();
    let mut families: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for name in names.iter().filter(|n| n.contains('.') && is_canonical(n)) {
        let (family, leaf) = family_of(name);
        let aliases: Vec<&str> =
            tool_aliases::aliases_of(name).into_iter().filter(|a| present.contains(a)).collect();
        let verb = if aliases.is_empty() { leaf } else { format!("{leaf} (alias {})", aliases.join(", ")) };
        families.entry(family).or_default().push(verb);
    }
    let mut starter_names: Vec<&str> = starters.iter().map(String::as_str).collect();
    starter_names.sort_unstable();
    starter_names.dedup();

    let mut out = String::new();
    out.push_str(&start_marker("verbs"));
    out.push_str("\n## Verbs\n\n");
    out.push_str("One line per family; `host.help.get(name)` describes a verb. Callable before you hold a key: ");
    out.push_str(&starter_names.iter().map(|s| format!("`{s}`")).collect::<Vec<_>>().join(", "));
    out.push('\n');
    for (family, mut verbs) in families {
        verbs.sort();
        out.push_str(&format!("- `{family}` ({}): {}\n", verbs.len(), verbs.join(", ")));
    }
    out.push_str(&end_marker("verbs"));
    out
}

/// The `limits` section: every key of `plan.ceilings_json()` (nested keys
/// dotted, `budget.max_wall_ms`) with its `free` value, then its `pro` one.
/// No number is typed here; each comes from the catalog's own plan rows.
pub fn render_limits(catalog: &crate::plans::PlanCatalog) -> String {
    let flat = |plan: &str| -> BTreeMap<String, i64> {
        catalog.get(plan).map(|p| p.ceilings_flat().into_iter().collect()).unwrap_or_default()
    };
    let (free, pro) = (flat("free"), flat("pro"));
    let mut out = String::new();
    out.push_str(&start_marker("limits"));
    out.push_str("\n## Limits\n\nCeilings the plan enforces, free then pro (a call over one is refused, never clipped; `billing.plans` has prices):\n\n");
    for (key, value) in &free {
        let pro_value = pro.get(key).map_or_else(|| "-".to_string(), i64::to_string);
        out.push_str(&format!("- `{key}`: free {value} · pro {pro_value}\n"));
    }
    for (key, value) in pro.iter().filter(|(k, _)| !free.contains_key(*k)) {
        out.push_str(&format!("- `{key}`: free - · pro {value}\n"));
    }
    out.push_str(&end_marker("limits"));
    out
}

/// The `errors` section: one bullet per [`crate::error_registry::ERROR_CODES`]
/// entry, `code (class): remedy`, in the registry's own (sorted) order.
pub fn render_errors() -> String {
    render_errors_from(crate::error_registry::ERROR_CODES.iter())
}

/// [`render_errors`] over any entry list (the tests append a fabricated row).
pub fn render_errors_from<'a>(entries: impl IntoIterator<Item = &'a crate::error_registry::ErrorEntry>) -> String {
    let mut out = String::new();
    out.push_str(&start_marker("errors"));
    out.push_str("\n## Errors\n\nWhat to do next for each code the host returns (`host.errors` has the full rows):\n\n");
    for e in entries {
        out.push_str(&format!("- `{}` ({}): {}\n", e.code, e.class.as_str(), e.remedy.replace('\n', " ")));
    }
    out.push_str(&end_marker("errors"));
    out
}

/// Every `host.*`/`billing.*` verb name a remedy text mentions.
pub fn verbs_named_in(remedy: &str) -> Vec<String> {
    let bytes = remedy.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let rest = &remedy[i..];
        let boundary = i == 0 || !(bytes[i - 1].is_ascii_alphanumeric() || bytes[i - 1] == b'_' || bytes[i - 1] == b'.');
        if boundary && (rest.starts_with("host.") || rest.starts_with("billing.")) {
            let end = rest
                .find(|c: char| !(c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '.'))
                .unwrap_or(rest.len());
            let name = rest[..end].trim_end_matches('.');
            if name.contains('.') && !name.ends_with('.') {
                out.push(name.to_string());
            }
            i += end.max(1);
        } else {
            i += 1;
        }
    }
    out
}

/// `(code, verb)` for every remedy that names a verb `names` does not carry.
pub fn unknown_remedy_verbs(names: &[String]) -> Vec<(String, String)> {
    unknown_remedy_verbs_in(crate::error_registry::ERROR_CODES.iter(), names)
}

/// [`unknown_remedy_verbs`] over any entry list.
pub fn unknown_remedy_verbs_in<'a>(
    entries: impl IntoIterator<Item = &'a crate::error_registry::ErrorEntry>,
    names: &[String],
) -> Vec<(String, String)> {
    let known: BTreeSet<&str> = names.iter().map(String::as_str).collect();
    let mut bad = Vec::new();
    for e in entries {
        for verb in verbs_named_in(e.remedy) {
            if !known.contains(verb.as_str()) {
                bad.push((e.code.to_string(), verb));
            }
        }
    }
    bad
}

/// First sentence of a clap `about`, without a leading `PRD-...:` tag.
fn about_of(cmd: &clap::Command) -> String {
    let about = cmd.get_about().map(|a| a.to_string()).unwrap_or_default();
    let about = match about.strip_prefix("PRD-").and_then(|rest| rest.split_once(": ")) {
        Some((_, rest)) => rest.to_string(),
        None => about,
    };
    about.split_once(". ").map_or(about.trim_end_matches('.').to_string(), |(first, _)| first.to_string())
}

/// `mcphost dev <DIR> [--plan <PLAN>] ...`, read off the clap definition.
fn usage_of(path: &str, cmd: &clap::Command) -> String {
    let mut parts = vec![path.to_string()];
    let mut flags = Vec::new();
    for a in cmd.get_arguments().filter(|a| !a.is_hide_set() && !matches!(a.get_id().as_str(), "help" | "version")) {
        let names = a
            .get_value_names()
            .map(|n| n.iter().map(|v| format!("<{v}>")).collect::<Vec<_>>().join(" "))
            .unwrap_or_else(|| format!("<{}>", a.get_id().as_str().to_uppercase()));
        if a.is_positional() {
            parts.push(if a.is_required_set() { names } else { format!("[{names}]") });
        } else if a.get_action().takes_values() {
            flags.push(format!("[--{} {names}]", a.get_long().unwrap_or(a.get_id().as_str())));
        } else {
            flags.push(format!("[--{}]", a.get_long().unwrap_or(a.get_id().as_str())));
        }
    }
    parts.extend(flags);
    parts.join(" ")
}

/// The `devloop` section: the local-loop verbs present in `cli` (`tool`,
/// `dev`, `push`), usage and one-line purpose read from its clap definition.
/// A verb the binary lacks is neither listed nor named; the section says
/// `no local verbs in this build` instead.
pub fn render_devloop(cli: &clap::Command) -> String {
    let bin = cli.get_name().to_string();
    let mut out = String::new();
    out.push_str(&start_marker("devloop"));
    out.push_str("\n## Local loop\n\nWork from a directory on disk before anything is published:\n\n");
    let mut absent = false;
    for verb in DEVLOOP_VERBS {
        let Some(sub) = cli.find_subcommand(verb) else {
            absent = true;
            continue;
        };
        let path = format!("{bin} {verb}");
        if sub.has_subcommands() {
            for inner in sub.get_subcommands().filter(|c| !c.is_hide_set() && c.get_name() != "help") {
                out.push_str(&format!("- `{}` -- {}\n", usage_of(&format!("{path} {}", inner.get_name()), inner), about_of(inner)));
            }
        } else {
            out.push_str(&format!("- `{}` -- {}\n", usage_of(&path, sub), about_of(sub)));
        }
    }
    if absent {
        out.push_str("- no local verbs in this build\n");
    }
    out.push_str(&end_marker("devloop"));
    out
}

/// The text of `section` (markers included) as it stands in `content`.
pub fn extract_section(content: &str, section: &str) -> Option<String> {
    let (start, end) = (start_marker(section), end_marker(section));
    let s = content.find(&start)?;
    let e = content.find(&end)? + end.len();
    (e > s).then(|| content[s..e].to_string())
}

/// Render `section` from the in-process registries; `devloop` needs the
/// binary's clap definition in `cli`.
pub fn render_section(section: &str, kinds: &KindRegistry, cli: Option<&clap::Command>) -> Result<String, String> {
    match section {
        "verbs" => Ok(render_verbs(&registry_tool_names(kinds), &crate::handler::starter_tool_names(kinds))),
        "limits" => Ok(render_limits(&crate::plans::PlanCatalog::default_catalog())),
        "errors" => Ok(render_errors()),
        "devloop" => cli.map(render_devloop).ok_or_else(|| "devloop needs the binary's CLI definition".to_string()),
        other => Err(format!("unknown section `{other}` (one of: {})", SECTIONS.join(", "))),
    }
}

/// Replace the one `section` block of `content` (markers included) with
/// `block`. The markers must each appear exactly once, start before end.
pub fn splice_section(content: &str, section: &str, block: &str) -> Result<String, String> {
    let (start, end) = (start_marker(section), end_marker(section));
    if content.matches(&start).count() != 1 || content.matches(&end).count() != 1 {
        return Err(format!("expected exactly one {start} / {end} pair"));
    }
    let s = content.find(&start).unwrap_or_default();
    let e = content.find(&end).unwrap_or_default() + end.len();
    if e <= s {
        return Err(format!("{end} precedes {start}"));
    }
    Ok(format!("{}{}{}", &content[..s], block.trim_end(), &content[e..]))
}

/// `existing` with every section of `sections` re-rendered by `render`.
pub fn render_file(
    existing: &str,
    sections: &[&str],
    render: &dyn Fn(&str) -> Result<String, String>,
) -> Result<String, String> {
    let mut out = existing.to_string();
    for section in sections {
        let block = render(section)?;
        out = splice_section(&out, section, &block).map_err(|e| format!("section `{section}`: {e}"))?;
    }
    Ok(out)
}

/// The first line (1-based, with its text) where `actual` differs from
/// `rendered`; `None` when they are byte-equal.
pub fn first_difference(actual: &str, rendered: &str) -> Option<(usize, String)> {
    if actual == rendered {
        return None;
    }
    let (mut a, mut r) = (actual.lines(), rendered.lines());
    let mut n = 0;
    loop {
        n += 1;
        match (a.next(), r.next()) {
            (Some(x), Some(y)) if x == y => {}
            (Some(x), _) => return Some((n, x.to_string())),
            (None, Some(y)) => return Some((n, format!("(missing; generated: {y})"))),
            (None, None) => return Some((n, "(trailing bytes differ)".to_string())),
        }
    }
}

/// `https://host/mcp` (or a tenant's `/u/<secret>/mcp`) -> the base `rpc_call` appends `/mcp` to.
fn base_of(url: &str) -> String {
    let url = url.trim_end_matches('/');
    url.strip_suffix("/mcp").unwrap_or(url).to_string()
}

/// Every tool name one `tools/list` of the host at `url` advertises
/// (following `nextCursor`), as `bearer` sees it.
pub async fn fetch_tool_names(url: &str, bearer: Option<&str>) -> Result<Vec<String>, String> {
    let http = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(60))
        .build()
        .map_err(|e| e.to_string())?;
    let base = base_of(url);
    let mut names = Vec::new();
    let mut cursor: Option<String> = None;
    for id in 1..=64u64 {
        let params = cursor.as_ref().map_or_else(|| serde_json::json!({}), |c| serde_json::json!({"cursor": c}));
        let req = crate::client::RpcRequest { method: "tools/list", params, id, bearer };
        let result = crate::client::rpc_call(&http, &base, req).await.map_err(|e| format!("tools/list at {url}: {e}"))?;
        let tools = result.get("tools").and_then(|t| t.as_array()).ok_or_else(|| format!("tools/list at {url} returned no tools array"))?;
        names.extend(tools.iter().filter_map(|t| t.get("name").and_then(|n| n.as_str()).map(str::to_string)));
        match result.get("nextCursor").and_then(|c| c.as_str()) {
            Some(next) => cursor = Some(next.to_string()),
            None => return Ok(names),
        }
    }
    Err(format!("tools/list at {url} kept paging past 64 pages"))
}

/// The `verbs` section derived from a running host's own `tools/list`: the
/// starter line from its anonymous listing, the families from the listing
/// `bearer` sees. Without a key an anonymous listing that is only the
/// starter set is refused, because it would render a skill that omits every
/// other verb.
pub async fn render_verbs_from_url(url: &str, bearer: Option<&str>) -> Result<String, String> {
    let starters = fetch_tool_names(url, None).await?;
    let names = match bearer {
        Some(_) => fetch_tool_names(url, bearer).await?,
        None => starters.clone(),
    };
    if bearer.is_none() && names.iter().filter(|n| n.contains('.') && is_canonical(n)).count() <= starters.len() {
        return Err(format!(
            "{url} lists only its anonymous starter set; pass --key (or set MCPHOST_TENANT_KEY) so tools/list shows every verb"
        ));
    }
    Ok(render_verbs(&names, &starters))
}
