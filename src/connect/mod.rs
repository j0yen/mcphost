//! PRD-mcphost-connection-provider-registry: `providers.toml` is the one
//! place an upstream OAuth provider is described. `build.rs` generates
//! [`Provider`], [`Provider::ALL`], [`Provider::parse`] and the static
//! [`SPECS`] table from it (via [`codegen`], the same code the tests re-run);
//! every surface that names a provider reads that table:
//!
//! 1. `host.connect.providers`' rows ([`providers_json`]),
//! 2. the `provider` JSON-schema `enum` ([`provider_enum`]),
//! 3. `docs/connections.md` ([`render_connections_doc`]),
//! 4. the `## Upstream connections` section of `www/llms.txt`
//!    ([`render_llms_section`] / [`splice_llms_section`]),
//! 5. the callback route table ([`callback_routes`]),
//!
//! plus [`Provider::validate_scopes`]. Each renderer takes a `&[ProviderSpec]`
//! so a test can feed it a table with an extra row.

use serde_json::{Value, json};
use std::collections::BTreeSet;

pub mod codegen;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refresh {
    Rotating,
    OfflineAccess,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScopeSpec {
    pub name: &'static str,
    pub purpose: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProviderSpec {
    pub name: &'static str,
    pub auth_url: &'static str,
    pub token_url: &'static str,
    pub scopes: &'static [ScopeSpec],
    pub refresh: Refresh,
    pub client_id_env: &'static str,
    pub client_secret_env: &'static str,
    pub extra_authorize_params: &'static [(&'static str, &'static str)],
    pub docs_url: &'static str,
}

include!(concat!(env!("OUT_DIR"), "/providers_gen.rs"));

/// The committed `providers.toml`, for tests that re-run the generator.
pub const PROVIDERS_TOML: &str = include_str!("../../providers.toml");

impl Provider {
    pub fn name(self) -> &'static str {
        self.spec().name
    }

    /// `Ok(())` when every scope is one of this provider's own scope names;
    /// otherwise `Err` with the unknown ones, in input order.
    pub fn validate_scopes(self, scopes: &[String]) -> Result<(), Vec<String>> {
        let known = self.spec().scopes;
        let bad: Vec<String> = scopes
            .iter()
            .filter(|s| !known.iter().any(|k| k.name == s.as_str()))
            .cloned()
            .collect();
        if bad.is_empty() { Ok(()) } else { Err(bad) }
    }
}

/// `"github, google, notion, slack"` -- the error text for an unknown preset.
pub fn provider_names_csv() -> String {
    Provider::ALL.iter().map(|p| p.name()).collect::<Vec<_>>().join(", ")
}

impl codegen::Row {
    /// Leak a parsed row into a `ProviderSpec` (tests only need a handful;
    /// the real table is the generated static).
    fn leak(&self) -> ProviderSpec {
        fn s(v: &str) -> &'static str {
            Box::leak(v.to_string().into_boxed_str())
        }
        let scopes: Vec<ScopeSpec> = self
            .scopes
            .iter()
            .map(|(n, p)| ScopeSpec { name: s(n), purpose: s(p) })
            .collect();
        let extra: Vec<(&'static str, &'static str)> = self
            .extra_authorize_params
            .iter()
            .map(|(k, v)| (s(k), s(v)))
            .collect();
        ProviderSpec {
            name: s(&self.name),
            auth_url: s(&self.auth_url),
            token_url: s(&self.token_url),
            scopes: Box::leak(scopes.into_boxed_slice()),
            refresh: match self.refresh.as_str() {
                "rotating" => Refresh::Rotating,
                "offline_access" => Refresh::OfflineAccess,
                _ => Refresh::None,
            },
            client_id_env: s(&self.client_id_env),
            client_secret_env: s(&self.client_secret_env),
            extra_authorize_params: Box::leak(extra.into_boxed_slice()),
            docs_url: s(&self.docs_url),
        }
    }
}

/// Specs for arbitrary `providers.toml` text (leaked; test use).
pub fn specs_from_toml(text: &str) -> Result<Vec<ProviderSpec>, String> {
    Ok(codegen::parse_table(text)?.iter().map(codegen::Row::leak).collect())
}

/// The compiled table as a slice.
pub fn all_specs() -> &'static [ProviderSpec] {
    &SPECS
}

// ---- availability ------------------------------------------------------

pub const REASON_CLIENT_ID: &str = "client_id_not_configured";
pub const REASON_CLIENT_SECRET: &str = "client_secret_not_configured";

/// Which providers the host has credentials for, snapshotted once at startup
/// (`AppState.connect_providers`). Holds only the verdict, never an env value.
#[derive(Debug, Clone, Default)]
pub struct ConnectProviders {
    /// `(provider name, reason it is unavailable)`; `None` means available.
    verdicts: Vec<(&'static str, Option<&'static str>)>,
}

impl ConnectProviders {
    pub fn from_env() -> Self {
        Self::from_lookup(all_specs(), |k| std::env::var(k).ok())
    }

    pub fn from_lookup(specs: &[ProviderSpec], lookup: impl Fn(&str) -> Option<String>) -> Self {
        let present = |k: &str| lookup(k).is_some_and(|v| !v.is_empty());
        let verdicts = specs
            .iter()
            .map(|s| {
                let reason = if !present(s.client_id_env) {
                    Some(REASON_CLIENT_ID)
                } else if !present(s.client_secret_env) {
                    Some(REASON_CLIENT_SECRET)
                } else {
                    None
                };
                (s.name, reason)
            })
            .collect();
        Self { verdicts }
    }

    pub fn reason(&self, name: &str) -> Option<&'static str> {
        self.verdicts.iter().find(|(n, _)| *n == name).and_then(|(_, r)| *r)
    }

    pub fn available(&self) -> Vec<&'static str> {
        self.verdicts.iter().filter(|(_, r)| r.is_none()).map(|(n, _)| *n).collect()
    }

    pub fn unavailable(&self) -> Vec<(&'static str, &'static str)> {
        self.verdicts.iter().filter_map(|(n, r)| r.map(|r| (*n, r))).collect()
    }
}

/// `host.connect.providers`: one row per [`Provider::ALL`] (or just the one
/// named by the optional `provider` argument).
pub fn providers_tool(state: &crate::state::AppState, args: &Value) -> Result<Value, crate::errors::AppError> {
    let rows = providers_json(all_specs(), &state.connect_providers);
    let rows: Vec<Value> = match args.get("provider") {
        None | Some(Value::Null) => rows,
        Some(Value::String(name)) => {
            let Some(p) = Provider::parse(name) else {
                return Err(crate::errors::AppError::InvalidParams(format!(
                    "unknown provider '{name}': expected one of {}",
                    provider_names_csv()
                )));
            };
            rows.into_iter().filter(|r| r["name"] == p.name()).collect()
        }
        Some(_) => {
            return Err(crate::errors::AppError::InvalidParams(
                "provider must be a string".to_string(),
            ));
        }
    };
    let count = rows.len();
    Ok(json!({"providers": rows, "count": count}))
}

// ---- surfaces ----------------------------------------------------------

/// Surface 1: one row per spec. Never carries a client id or secret.
pub fn providers_json(specs: &[ProviderSpec], status: &ConnectProviders) -> Vec<Value> {
    specs
        .iter()
        .map(|s| {
            let mut row = json!({
                "name": s.name,
                "scopes": s.scopes.iter().map(|x| x.name).collect::<Vec<_>>(),
                "scope_purposes": s.scopes.iter().map(|x| (x.name.to_string(), json!(x.purpose))).collect::<serde_json::Map<_, _>>(),
                "refresh": s.refresh.as_str(),
                "docs_url": s.docs_url,
                "available": status.reason(s.name).is_none(),
            });
            if let (Some(reason), Value::Object(m)) = (status.reason(s.name), &mut row) {
                m.insert("reason".to_string(), json!(reason));
            }
            row
        })
        .collect()
}

/// Surface 2: the `provider` schema enum, the way `publish_kind_enum` serves kinds.
pub fn provider_enum() -> Vec<&'static str> {
    Provider::ALL.iter().map(|p| p.name()).collect()
}

/// Surface 5: the callback route table (PRD 2 mounts one route per row).
pub fn callback_routes(specs: &[ProviderSpec]) -> Vec<String> {
    specs.iter().map(|s| format!("/connect/{}/callback", s.name)).collect()
}

fn scope_cell(s: &str) -> String {
    s.replace('|', "\\|")
}

/// Surface 3: `docs/connections.md`.
pub fn render_connections_doc(specs: &[ProviderSpec]) -> String {
    let mut out = String::new();
    out.push_str("# Upstream connections\n\n");
    out.push_str(
        "Generated by `mcphost gen-docs` from `providers.toml` -- do not hand-edit; add a \
         provider by adding a block to `providers.toml`. `host.connect.providers` lists \
         these providers with `available: true|false` (a provider is available once the \
         host has both its client id and client secret in the environment).\n\n",
    );
    for s in specs {
        out.push_str(&format!("## `{}`\n\n", s.name));
        out.push_str(&format!("- authorize: {}\n", s.auth_url));
        out.push_str(&format!("- token: {}\n", s.token_url));
        out.push_str(&format!("- refresh: `{}`\n", s.refresh.as_str()));
        out.push_str(&format!("- client id env: `{}`\n", s.client_id_env));
        out.push_str(&format!("- client secret env: `{}`\n", s.client_secret_env));
        out.push_str(&format!("- callback: `/connect/{}/callback`\n", s.name));
        out.push_str(&format!("- docs: {}\n", s.docs_url));
        for (k, v) in s.extra_authorize_params {
            out.push_str(&format!("- authorize param: `{k}={v}`\n"));
        }
        out.push('\n');
        if s.scopes.is_empty() {
            out.push_str("No scope names; access is granted by the user at authorization time.\n\n");
        } else {
            out.push_str("| scope | purpose |\n|---|---|\n");
            for sc in s.scopes {
                out.push_str(&format!("| `{}` | {} |\n", scope_cell(sc.name), scope_cell(sc.purpose)));
            }
            out.push('\n');
        }
    }
    out
}

pub const LLMS_SECTION_START: &str = "<!-- connections:start -->";
pub const LLMS_SECTION_END: &str = "<!-- connections:end -->";

/// Surface 4: the `## Upstream connections` section of `www/llms.txt`.
pub fn render_llms_section(specs: &[ProviderSpec]) -> String {
    let mut out = String::new();
    out.push_str(LLMS_SECTION_START);
    out.push_str("\n## Upstream connections\n\n");
    out.push_str(
        "Generated by `mcphost gen-docs` from `providers.toml` -- do not hand-edit. Call \
         `host.connect.providers` to see which providers this host can connect right now \
         (`available`, and `reason` when not) and the exact scope names each accepts; \
         the full tables are in docs/connections.md.\n\n",
    );
    for s in specs {
        out.push_str(&format!(
            "- `{}` -- refresh `{}`, {} scope(s)\n",
            s.name,
            s.refresh.as_str(),
            s.scopes.len()
        ));
    }
    out.push_str(LLMS_SECTION_END);
    out
}

/// Replace the marked section, or on a first run insert it right before the
/// `## Full documentation` heading (appending if there is none).
pub fn splice_llms_section(content: &str, section: &str) -> String {
    let section = section.trim_end();
    if let (Some(start), Some(end_pos)) = (content.find(LLMS_SECTION_START), content.find(LLMS_SECTION_END)) {
        let end = end_pos + LLMS_SECTION_END.len();
        format!("{}{}{}", &content[..start], section, &content[end..])
    } else if let Some(idx) = content.find("## Full documentation") {
        format!("{}{}\n\n{}", &content[..idx], section, &content[idx..])
    } else {
        format!("{}\n\n{}\n", content.trim_end(), section)
    }
}

// ---- reading surfaces back (drift test, doc-check) ---------------------

/// Provider names `docs/connections.md` declares: its `## \`name\`` headings.
pub fn doc_provider_names(doc: &str) -> BTreeSet<String> {
    doc.lines()
        .filter_map(|l| l.strip_prefix("## `")?.strip_suffix('`'))
        .map(str::to_string)
        .collect()
}

/// Provider names the llms.txt connections section lists: its `` - `name` `` bullets.
pub fn llms_provider_names(llms: &str) -> BTreeSet<String> {
    let (Some(start), Some(end)) = (llms.find(LLMS_SECTION_START), llms.find(LLMS_SECTION_END)) else {
        return BTreeSet::new();
    };
    if end < start {
        return BTreeSet::new();
    }
    llms[start..end]
        .lines()
        .filter_map(|l| l.strip_prefix("- `")?.split('`').next())
        .map(str::to_string)
        .collect()
}

/// Provider names in a callback route table (`/connect/<name>/callback`).
pub fn route_provider_names(routes: &[String]) -> BTreeSet<String> {
    routes
        .iter()
        .filter_map(|r| r.strip_prefix("/connect/")?.strip_suffix("/callback"))
        .map(str::to_string)
        .collect()
}
