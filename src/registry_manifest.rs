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

/// The registry lookup the dry-run and `--live` both query, and the one URL
/// `README.md` and `docs/launch/wave1-checklist.md` link (checked by
/// `regone_ac06`). It is `docs/clients.toml`'s `[registry] search_url`, not a
/// second spelling of it: the code reads it through
/// [`crate::clients::builtin`] (see [`registry_search_url_from_env`]), this
/// const exists only because `regone_ac06` needs a literal to search the docs
/// for, and `registry_search_url_const_is_clients_toml_value` pins the two
/// together so they cannot drift. Overridable per run with
/// `$MCPHOST_REGISTRY_SEARCH_URL` (tests point it at a stub).
pub const REGISTRY_SEARCH_URL: &str =
    "https://registry.modelcontextprotocol.io/v0.1/servers/dev.mcphost%2Fmcphost/versions/latest";

/// Per-side network timeout for `--live`; the two probes run concurrently,
/// so the whole check stays inside ~10 s.
pub const LIVE_TIMEOUT_SECS: u64 = 10;

/// `$MCPHOST_REGISTRY_SEARCH_URL`, defaulting to `docs/clients.toml`'s
/// `[registry] search_url` (the value [`REGISTRY_SEARCH_URL`] mirrors).
pub fn registry_search_url_from_env() -> String {
    std::env::var("MCPHOST_REGISTRY_SEARCH_URL")
        .unwrap_or_else(|_| crate::clients::builtin().registry.search_url.clone())
}

/// Outcome of `registry-manifest --live`.
#[derive(Debug, Clone)]
pub struct LiveReport {
    pub registry_version: Option<String>,
    pub live_version: Option<String>,
    pub remotes: Option<String>,
    /// `Some(true|false)` when both sides answered, `None` = unknown.
    pub drift: Option<bool>,
    /// Why `drift` is unknown.
    pub reason: Option<String>,
}

impl LiveReport {
    /// Process exit code: 0 no drift, 3 drift, 2 unknown.
    pub fn exit_code(&self) -> i32 {
        match self.drift {
            Some(false) => 0,
            Some(true) => 3,
            None => 2,
        }
    }

    fn drift_word(&self) -> &'static str {
        match self.drift {
            Some(true) => "yes",
            Some(false) => "no",
            None => "unknown",
        }
    }

    /// `registry=<v> live=<v> remotes=<url> drift=<yes|no|unknown>`
    /// (+ ` reason=...` when unknown).
    pub fn line(&self) -> String {
        let or_q = |o: &Option<String>| o.clone().unwrap_or_else(|| "?".to_string());
        let mut s = format!(
            "registry={} live={} remotes={} drift={}",
            or_q(&self.registry_version),
            or_q(&self.live_version),
            or_q(&self.remotes),
            self.drift_word()
        );
        if let Some(r) = &self.reason {
            s.push_str(&format!(" reason={r}"));
        }
        s
    }

    /// `{registry_version, live_version, remotes, drift, checked_at}`.
    pub fn to_json(&self, checked_at: &str) -> Value {
        json!({
            "registry_version": self.registry_version,
            "live_version": self.live_version,
            "remotes": self.remotes,
            "drift": self.drift_word(),
            "reason": self.reason,
            "checked_at": checked_at,
        })
    }
}

fn version_key(v: &str) -> Vec<u64> {
    v.split(['.', '-', '+'])
        .map(|p| p.parse::<u64>().unwrap_or(0))
        .collect()
}

/// Picks the entry named `name` from a registry response, preferring the
/// highest version. Two shapes are accepted: the list the old `/v0/servers`
/// search returned (`{"servers": [{"server": ..}, ..]}`, every published
/// version) and the single entry the `/v0.1/servers/<name>/versions/latest`
/// lookup returns (`{"server": {..}, "_meta": {..}}`). Returns
/// `(version, first remote url)`.
pub fn select_registry_entry(body: &Value, name: &str) -> Option<(String, Option<String>)> {
    let single;
    let servers: &[Value] = match body.get("servers").and_then(Value::as_array) {
        Some(list) => list,
        None if body.get("server").is_some() => {
            single = [body.clone()];
            &single
        }
        None => return None,
    };
    servers
        .iter()
        .filter_map(|s| {
            let server = s.get("server").unwrap_or(s);
            if server.get("name")?.as_str()? != name {
                return None;
            }
            let version = server.get("version")?.as_str()?.to_string();
            let remote = server
                .get("remotes")
                .and_then(|r| r.get(0))
                .and_then(|r| r.get("url"))
                .and_then(|u| u.as_str())
                .map(str::to_string);
            Some((version, remote))
        })
        .max_by_key(|(v, _)| version_key(v))
}

/// Pulls `result.serverInfo.version` out of an `initialize` reply that is
/// either plain JSON or an SSE stream (`data: {...}` lines).
pub fn parse_initialize_version(body: &str) -> Option<String> {
    let from_json = |s: &str| -> Option<String> {
        let v: Value = serde_json::from_str(s.trim()).ok()?;
        v.pointer("/result/serverInfo/version")?.as_str().map(str::to_string)
    };
    from_json(body).or_else(|| {
        body.lines()
            .filter_map(|l| l.strip_prefix("data:"))
            .find_map(from_json)
    })
}

async fn fetch_registry(
    client: &reqwest::Client,
    search_url: &str,
) -> Result<(String, Option<String>), String> {
    let resp = client
        .get(search_url)
        .send()
        .await
        .map_err(|e| format!("registry unreachable: {}", short_err(&e)))?;
    if !resp.status().is_success() {
        return Err(format!("registry answered HTTP {}", resp.status().as_u16()));
    }
    let body: Value = resp
        .json()
        .await
        .map_err(|e| format!("registry body not JSON: {}", short_err(&e)))?;
    select_registry_entry(&body, &full_name())
        .ok_or_else(|| format!("registry has no entry named {}", full_name()))
}

async fn fetch_live(client: &reqwest::Client, endpoint: &str) -> Result<String, String> {
    let init = json!({
        "jsonrpc": "2.0", "id": 1, "method": "initialize",
        "params": {
            "protocolVersion": "2025-06-18",
            "capabilities": {},
            "clientInfo": {"name": "mcphost-registry-probe", "version": "0"},
        },
    });
    let resp = client
        .post(endpoint)
        .header("Accept", "application/json, text/event-stream")
        .header("X-Mcphost-Origin", "probe")
        .json(&init)
        .send()
        .await
        .map_err(|e| format!("endpoint unreachable: {}", short_err(&e)))?;
    if !resp.status().is_success() {
        return Err(format!("endpoint answered HTTP {}", resp.status().as_u16()));
    }
    let text = resp
        .text()
        .await
        .map_err(|e| format!("endpoint body unreadable: {}", short_err(&e)))?;
    parse_initialize_version(&text)
        .ok_or_else(|| "endpoint initialize carried no serverInfo.version".to_string())
}

fn short_err(e: &reqwest::Error) -> String {
    if e.is_timeout() {
        format!("timed out after {LIVE_TIMEOUT_SECS}s")
    } else if e.is_connect() {
        "connection failed".to_string()
    } else {
        "request failed".to_string()
    }
}

/// `registry-manifest --live`: compares the public registry's published
/// version to the version the live endpoint's `initialize` reports. Both
/// probes run concurrently with a [`LIVE_TIMEOUT_SECS`] timeout each.
pub async fn live_check(public_url: &str, search_url: &str) -> LiveReport {
    let endpoint = format!("{}/mcp", public_url.trim_end_matches('/'));
    let client = match reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(LIVE_TIMEOUT_SECS))
        .build()
    {
        Ok(c) => c,
        Err(_) => {
            return LiveReport {
                registry_version: None,
                live_version: None,
                remotes: None,
                drift: None,
                reason: Some("http client build failed".to_string()),
            };
        }
    };
    let (reg, live) = tokio::join!(
        fetch_registry(&client, search_url),
        fetch_live(&client, &endpoint)
    );
    let mut reasons = Vec::new();
    let (registry_version, remotes) = match reg {
        Ok((v, r)) => (Some(v), r),
        Err(e) => {
            reasons.push(e);
            (None, None)
        }
    };
    let live_version = match live {
        Ok(v) => Some(v),
        Err(e) => {
            reasons.push(e);
            None
        }
    };
    let drift = match (&registry_version, &live_version) {
        (Some(a), Some(b)) => Some(a != b),
        _ => None,
    };
    LiveReport {
        registry_version,
        live_version,
        remotes: remotes.or(Some(endpoint)),
        drift,
        reason: if reasons.is_empty() { None } else { Some(reasons.join("; ").replace(' ', "_")) },
    }
}

/// Validates a registry entry against the pinned schema plus this host's
/// own "a listing must name a remote" rule (`required: ["remotes"]`, layered
/// via `allOf` because the upstream schema leaves `remotes` optional). The
/// one validator `--validate` and the `reglist_ac03` suite both stand for.
pub fn validate_entry(entry: &Value, schema: &Value) -> Result<(), String> {
    let composed = json!({
        "definitions": schema["definitions"],
        "allOf": [
            {"$ref": "#/definitions/ServerDetail"},
            {"required": ["remotes"]},
        ],
    });
    let validator = jsonschema::validator_for(&composed).map_err(|e| format!("schema: {e}"))?;
    match validator.iter_errors(entry).next() {
        Some(err) => Err(format!("{err} at {}", err.instance_path)),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The const and `docs/clients.toml`'s `[registry] search_url` are one
    /// value: the const is only the literal `regone_ac06` greps the docs
    /// for, so it must never drift from the toml the code actually reads.
    #[test]
    fn registry_search_url_const_is_clients_toml_value() {
        assert_eq!(REGISTRY_SEARCH_URL, crate::clients::builtin().registry.search_url);
    }

    #[test]
    fn select_registry_entry_accepts_single_entry_and_list() {
        let single = json!({
            "server": {"name": "dev.mcphost/mcphost", "version": "0.108.169",
                       "remotes": [{"type": "streamable-http", "url": "https://mcphost.dev/mcp"}]},
            "_meta": {}
        });
        assert_eq!(
            select_registry_entry(&single, "dev.mcphost/mcphost"),
            Some(("0.108.169".to_string(), Some("https://mcphost.dev/mcp".to_string())))
        );
        assert_eq!(select_registry_entry(&single, "other/thing"), None);
        let list = json!({"servers": [
            {"server": {"name": "dev.mcphost/mcphost", "version": "0.97.0"}},
            {"server": {"name": "dev.mcphost/mcphost", "version": "0.98.0"}},
        ]});
        assert_eq!(
            select_registry_entry(&list, "dev.mcphost/mcphost"),
            Some(("0.98.0".to_string(), None))
        );
    }
}
