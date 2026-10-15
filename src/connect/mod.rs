//! PRD-mcphost-connection-provider-registry: the upstream OAuth providers a
//! host can connect, as ONE table generated from `providers.toml` by
//! `build.rs`. Every surface that names a provider reads [`Provider::ALL`]:
//! `host.connect.providers`, the `provider` schema enum, `docs/connections.md`,
//! the `## Upstream connections` section of `www/llms.txt`, scope validation
//! and the callback route table. `tests/cprov_ac02_*` fails if any of them
//! could name a different set.

use serde_json::{Value, json};

/// One scope a provider accepts: `name` is the exact string sent upstream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Scope {
    pub name: &'static str,
    pub purpose: &'static str,
}

/// How a provider's tokens expire.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refresh {
    /// Each refresh returns a new refresh token that replaces the stored one.
    Rotating,
    /// A refresh token is issued once (Google: `access_type=offline`).
    OfflineAccess,
    /// Tokens never expire: `expires_unix` is stored as `i64::MAX`.
    None,
}

impl Refresh {
    pub fn as_str(self) -> &'static str {
        match self {
            Refresh::Rotating => "rotating",
            Refresh::OfflineAccess => "offline_access",
            Refresh::None => "none",
        }
    }
}

/// One `providers.toml` row, generated.
#[derive(Debug)]
pub struct ProviderSpec {
    pub name: &'static str,
    pub auth_url: &'static str,
    pub token_url: &'static str,
    pub scopes: &'static [Scope],
    pub refresh: Refresh,
    pub client_id_env: &'static str,
    pub client_secret_env: &'static str,
    pub extra_authorize_params: &'static [(&'static str, &'static str)],
    pub docs_url: &'static str,
    /// Compiled in only under the `test-support` feature.
    pub test_only: bool,
}

/// A handle on one generated [`ProviderSpec`]; `Provider::GITHUB.token_url`
/// reads through to the row.
#[derive(Debug, Clone, Copy)]
pub struct Provider(&'static ProviderSpec);

impl std::ops::Deref for Provider {
    type Target = ProviderSpec;
    fn deref(&self) -> &ProviderSpec {
        self.0
    }
}

impl PartialEq for Provider {
    fn eq(&self, other: &Provider) -> bool {
        self.name == other.name
    }
}

include!(concat!(env!("OUT_DIR"), "/providers_gen.rs"));

/// Why a provider cannot be used yet.
pub const REASON_CLIENT_ID: &str = "client_id_not_configured";
pub const REASON_CLIENT_SECRET: &str = "client_secret_not_configured";

/// Route a provider's OAuth callback is mounted at (PRD 2 mounts one per
/// [`Provider::ALL`]).
pub const CALLBACK_ROUTE_PREFIX: &str = "/connect/callback/";

impl Provider {
    /// The provider named `name`, if it is in [`Provider::ALL`].
    pub fn parse(name: &str) -> Option<Provider> {
        Provider::ALL.iter().copied().find(|p| p.name == name)
    }

    /// Every row a production build ships (no `test_only` row).
    pub fn public() -> impl Iterator<Item = Provider> {
        Provider::ALL.iter().copied().filter(|p| !p.test_only)
    }

    /// `Ok` when every scope is one of this provider's; else the unknown
    /// scope names, in input order.
    pub fn validate_scopes(&self, scopes: &[String]) -> Result<(), Vec<String>> {
        let unknown: Vec<String> = scopes
            .iter()
            .filter(|s| !self.scopes.iter().any(|known| known.name == s.as_str()))
            .cloned()
            .collect();
        if unknown.is_empty() { Ok(()) } else { Err(unknown) }
    }

    /// `None` when usable; else the reason. Both env vars must be present and
    /// non-empty; the id is checked first.
    pub fn unavailable_reason(&self, env: &dyn Fn(&str) -> Option<String>) -> Option<&'static str> {
        let set = |key: &str| env(key).is_some_and(|v| !v.is_empty());
        if !set(self.client_id_env) {
            Some(REASON_CLIENT_ID)
        } else if !set(self.client_secret_env) {
            Some(REASON_CLIENT_SECRET)
        } else {
            None
        }
    }

    /// The path this provider's OAuth callback is mounted at.
    pub fn callback_route(&self) -> String {
        format!("{CALLBACK_ROUTE_PREFIX}{}", self.name)
    }
}

/// The `provider` JSON-schema enum every `host.connect.*` tool carries (same
/// role as `KindRegistry::publish_kind_enum`).
pub fn provider_enum() -> Vec<&'static str> {
    Provider::ALL.iter().map(|p| p.name).collect()
}

/// The callback route table: one route per [`Provider::ALL`].
pub fn callback_route_table() -> Vec<String> {
    Provider::ALL.iter().map(|p| p.callback_route()).collect()
}

/// `host.connect.providers`: one row per provider (or just `only`). Never
/// carries the client id or secret -- only whether they are configured.
pub fn providers_json(only: Option<&str>, env: &dyn Fn(&str) -> Option<String>) -> Value {
    let rows: Vec<Value> = Provider::ALL
        .iter()
        .filter(|p| only.is_none_or(|n| p.name == n))
        .map(|p| {
            let reason = p.unavailable_reason(env);
            let mut row = json!({
                "name": p.name,
                "scopes": p.scopes.iter().map(|s| json!({"name": s.name, "purpose": s.purpose})).collect::<Vec<_>>(),
                "refresh": p.refresh.as_str(),
                "docs_url": p.docs_url,
                "available": reason.is_none(),
            });
            if let Some(r) = reason {
                row["reason"] = json!(r);
            }
            row
        })
        .collect();
    json!({ "providers": rows })
}

fn escape_cell(s: &str) -> String {
    s.replace('|', "\\|")
}

/// `docs/connections.md`, over `providers`.
pub fn render_connections_doc(providers: &[Provider]) -> String {
    let mut out = String::from(
        "# Upstream connections\n\nGenerated by `mcphost gen-docs` from `providers.toml` -- the same table \
         `host.connect.providers` is built from; do not hand-edit.\n\n",
    );
    out.push_str("| provider | refresh | docs |\n|---|---|---|\n");
    for p in providers {
        out.push_str(&format!("| `{}` | {} | {} |\n", p.name, p.refresh.as_str(), p.docs_url));
    }
    for p in providers {
        out.push_str(&format!("\n## `{}`\n\n", p.name));
        out.push_str(&format!(
            "Refresh: `{}`. Host credentials: `{}` and `{}`.\n\n",
            p.refresh.as_str(),
            p.client_id_env,
            p.client_secret_env
        ));
        if p.scopes.is_empty() {
            out.push_str("No scopes.\n");
        } else {
            out.push_str("| scope | purpose |\n|---|---|\n");
            for s in p.scopes {
                out.push_str(&format!("| `{}` | {} |\n", s.name, escape_cell(s.purpose)));
            }
        }
    }
    out
}

pub const LLMS_SECTION_START: &str = "<!-- connections:start -->";
pub const LLMS_SECTION_END: &str = "<!-- connections:end -->";
const LLMS_HEADING: &str = "## Upstream connections";

/// The marked `## Upstream connections` section of `www/llms.txt`.
pub fn render_llms_section(providers: &[Provider]) -> String {
    let mut out = format!("{LLMS_SECTION_START}\n{LLMS_HEADING}\n\n");
    out.push_str(
        "Generated by `mcphost gen-docs` from `providers.toml`. `host.connect.providers` lists each \
         provider with `available` and, when false, the `reason`; `provider` in every `host.connect.*` \
         tool is one of:\n\n",
    );
    for p in providers {
        let scopes = p.scopes.iter().map(|s| format!("`{}`", s.name)).collect::<Vec<_>>().join(", ");
        out.push_str(&format!(
            "- `{}` (refresh: {}; scopes: {})\n",
            p.name,
            p.refresh.as_str(),
            if scopes.is_empty() { "none".to_string() } else { scopes }
        ));
    }
    out.push_str(LLMS_SECTION_END);
    out
}

/// Replace the marked section in `content`, or -- before any marker exists --
/// insert it after the `## Upstream token vault` section (else append).
pub fn splice_llms_section(content: &str, section: &str) -> String {
    let section = section.trim_end();
    if let (Some(start), Some(end_pos)) = (content.find(LLMS_SECTION_START), content.find(LLMS_SECTION_END)) {
        let end = end_pos + LLMS_SECTION_END.len();
        return format!("{}{}{}", &content[..start], section, &content[end..]);
    }
    if let Some(vault) = content.find("## Upstream token vault") {
        let after = vault + "## Upstream token vault".len();
        if let Some(next) = content[after..].find("\n## ") {
            let idx = after + next + 1;
            return format!("{}{}\n\n{}", &content[..idx], section, &content[idx..]);
        }
    }
    format!("{}\n\n{}\n", content.trim_end(), section)
}

/// Provider names listed in a rendered `docs/connections.md` (its `## `name``
/// headings).
pub fn names_in_doc(doc: &str) -> Vec<String> {
    doc.lines()
        .filter_map(|l| l.strip_prefix("## `")?.strip_suffix('`'))
        .map(String::from)
        .collect()
}

/// Provider names listed in the marked llms.txt section (its `- `name` (`
/// bullets).
pub fn names_in_llms_section(content: &str) -> Vec<String> {
    let (Some(start), Some(end)) = (content.find(LLMS_SECTION_START), content.find(LLMS_SECTION_END)) else {
        return Vec::new();
    };
    content[start..end]
        .lines()
        .filter_map(|l| l.strip_prefix("- `")?.split_once('`'))
        .map(|(n, _)| n.to_string())
        .collect()
}

/// The surface check behind `tests/cprov_ac02_*` and
/// `scripts/connections-doc-check.sh`: every `(surface, names)` must equal
/// `expected`. The error names the surface and the extra (or missing) name.
pub fn check_surfaces(expected: &[String], surfaces: &[(&str, Vec<String>)]) -> Result<(), String> {
    for (surface, names) in surfaces {
        if let Some(extra) = names.iter().find(|n| !expected.contains(n)) {
            return Err(format!("surface {surface} names provider '{extra}' that is not in providers.toml"));
        }
        if let Some(missing) = expected.iter().find(|n| !names.contains(n)) {
            return Err(format!("surface {surface} is missing provider '{missing}' from providers.toml"));
        }
    }
    Ok(())
}
