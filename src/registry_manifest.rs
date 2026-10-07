//! `mcphost registry-manifest` (PRD-mcphost-registry-listing P0
//! requirements 1/2): generates the public MCP registry's `server.json`
//! listing from the same `Cargo.toml` constants the binary itself is built
//! from, plus the public endpoint this host actually serves, so the
//! listing can never disagree with the running server (technical
//! considerations).
//!
//! Distinct from [`crate::registry`]'s `host.registry_publish` -- that is
//! a tenant-facing tool publishing a TENANT's own domain-verified
//! namespace entry (PRD-mcphost-registry-publish); this module's
//! `server.json` describes mcphost itself, under the namespace Joe decided
//! (PRD's Open Questions, 2026-10-06): `dev.mcphost`, the reverse-DNS form
//! of `mcphost.dev`.

use serde_json::{Value, json};

/// The domain-verified namespace decided 2026-10-06 (PRD's Open
/// Questions): `dev.mcphost` is the reverse-DNS form of `mcphost.dev`. The
/// registry schema requires `name` to be `<namespace>/<server-name>`
/// (exactly one `/`), so this is paired with `Cargo.toml`'s own package
/// name, never spelled as a second literal.
pub const NAMESPACE: &str = "dev.mcphost";

/// This repo's own GitHub URL -- not itself a `Cargo.toml` field (the
/// package table carries no `repository` key), so named here instead of
/// invented per call site.
pub const REPOSITORY_URL: &str = "https://github.com/j0yen/mcphost";

/// Where the committed artifact lives, relative to the repo root.
pub const MANIFEST_PATH: &str = "registry/server.json";

/// The pinned upstream schema `--check`/the test suite validate against,
/// relative to the repo root. Fetched by hand from [`SCHEMA_URL`] (not
/// re-fetched at every build); see that file's own `$comment`.
pub const SCHEMA_PATH: &str = "registry/schema.json";

/// Recorded in `registry/schema.json`'s own `$comment` field too -- kept
/// here as well so a future re-fetch has one source for the URL instead of
/// two to keep in sync.
pub const SCHEMA_URL: &str =
    "https://static.modelcontextprotocol.io/schemas/2025-09-29/server.schema.json";

/// `name`'s required `<namespace>/<server-name>` shape, built from
/// [`NAMESPACE`] and `Cargo.toml`'s own package name -- the one place that
/// concatenation happens, so [`build_manifest`] and a test asserting on it
/// can never drift apart by each spelling the literal independently.
pub fn full_name() -> String {
    format!("{NAMESPACE}/{}", env!("CARGO_PKG_NAME"))
}

/// `$MCPHOST_PUBLIC_URL`, defaulting to the production host. Deliberately
/// a different default than `mcphost serve`'s own `http://{bind}` fallback
/// in `main.rs`: that one exists for a freshly-bound socket with no public
/// DNS yet, whereas this subcommand's whole job is describing the
/// already-public listing -- an unset env var here means "the production
/// default", not "whatever address I just bound".
pub fn public_url_from_env() -> String {
    std::env::var("MCPHOST_PUBLIC_URL").unwrap_or_else(|_| "https://mcphost.dev".to_string())
}

/// Builds the registry entry. `name`/`description`/`version`/`license`
/// come straight from `Cargo.toml` (requirement 2's "same constants");
/// `remotes[0].url` is `public_url` + `/mcp` (AC1), `trim_end_matches('/')`
/// first so a trailing slash on `$MCPHOST_PUBLIC_URL` can't double it (same
/// guard `reach.rs::fallback_block` already applies to a public URL before
/// appending a path).
pub fn build_manifest(public_url: &str) -> Value {
    let endpoint = format!("{}/mcp", public_url.trim_end_matches('/'));
    json!({
        "$schema": SCHEMA_URL,
        "name": full_name(),
        "description": env!("CARGO_PKG_DESCRIPTION"),
        "version": env!("CARGO_PKG_VERSION"),
        "license": env!("CARGO_PKG_LICENSE"),
        "repository": {
            "url": REPOSITORY_URL,
            "source": "github",
        },
        "websiteUrl": public_url,
        "remotes": [
            {"type": "streamable-http", "url": endpoint}
        ],
    })
}

/// Pretty-printed with a trailing newline -- the exact bytes `--write`
/// commits and `--check` compares the committed file against.
pub fn render(manifest: &Value) -> String {
    let mut s = serde_json::to_string_pretty(manifest).expect("serialize registry manifest");
    s.push('\n');
    s
}

/// Whether `committed` (the file `--check` read off disk) and `rendered`
/// (a fresh [`render`] of [`build_manifest`]) describe the same registry
/// entry once the `version` field is normalized out of both sides.
///
/// `--check` uses this instead of a byte comparison: a release land bumps
/// `Cargo.toml`'s `version` *after* this file is committed (this
/// subcommand's own `version` field comes straight from
/// `CARGO_PKG_VERSION`), so a byte-for-byte compare would make every
/// land's final gate fail on this file alone, by construction, every
/// time. The registry workflow (`.github/workflows/registry.yml`)
/// regenerates the file with `--write` on the `v*` tag job, so the
/// published registry entry still carries the real, matching version --
/// only the repo's committed copy is deliberately version-stale between
/// a PRD land and the next tag.
///
/// Any other drift (description, name, license, repository, remotes,
/// ...) still counts: only the `version` key is removed before the
/// comparison. A JSON parse failure on either side falls back to a plain
/// string compare, so a corrupt or missing committed file still reports
/// drift instead of silently matching.
pub fn matches_ignoring_version(committed: &str, rendered: &str) -> bool {
    fn without_version(s: &str) -> Option<Value> {
        let mut v: Value = serde_json::from_str(s).ok()?;
        v.as_object_mut()?.remove("version");
        Some(v)
    }
    match (without_version(committed), without_version(rendered)) {
        (Some(a), Some(b)) => a == b,
        _ => committed == rendered,
    }
}
