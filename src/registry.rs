//! `host.registry_publish` support (PRD requirement 15 / AC19): building an
//! MCP registry `server.json` document and naming the registry API base
//! URL a `--registry-url` / `$MCPHOST_REGISTRY_URL` flag enables.
//!
//! What this module deliberately does NOT do: decide the domain-namespace
//! VERIFICATION METHOD (DNS vs HTTP record). That is the PRD's Open
//! Questions item owned by Joe. Here, "verified" is only ever a per-tenant
//! boolean an admin sets via `admin.tenant_verify_namespace`
//! (`control::registry_publish` checks it); the method that convinces an
//! admin to set it stays entirely outside this crate.

use serde_json::{Value, json};

/// Off by default (PRD requirement 15's feature flag). Present on
/// [`crate::state::AppState`] only when `--registry-url` /
/// `$MCPHOST_REGISTRY_URL` was given at `serve` start.
#[derive(Clone, Debug)]
pub struct RegistryConfig {
    /// Base URL of the registry API (e.g.
    /// `https://registry.modelcontextprotocol.io`).
    /// `host.registry_publish` POSTs the `server.json` document to
    /// `<base_url>/v0/publish` (API v0.1, per the PRD's technical
    /// considerations).
    pub base_url: String,
}

/// Build the `server.json` document both the MCP registry and this host's
/// own `/.well-known/mcp/<namespace>/server.json` route serve: `name` (the
/// admin-verified, reverse-DNS-style domain namespace), `description`,
/// `version`, and a `remotes` entry naming this endpoint's streamable-HTTP
/// URL. Field names follow the MCP registry's `server.json` shape as
/// closely as this crate needs (PRD requirement 15).
pub fn build_server_json(domain_namespace: &str, display_name: &str, endpoint_url: &str) -> Value {
    json!({
        "name": domain_namespace,
        "description": format!(
            "mcphost tenant '{display_name}' -- tools published through host.tool_publish"
        ),
        "version": env!("CARGO_PKG_VERSION"),
        "remotes": [
            {"type": "streamable-http", "url": endpoint_url}
        ],
    })
}
