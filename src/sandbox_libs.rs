//! PRD-mcphost-sandbox-file-processing-libs: the libraries every python
//! tool may import with no `requirements` line. `sandbox-libs.toml` is the
//! one source; `sandbox-libs.lock` is its hashed `uv` lock. The shared base
//! env, the `gen-docs` table, the `unknown_import` hint and the publish-time
//! fast path all read from the same parsed rows, so none keeps its own copy.

use std::sync::OnceLock;

use serde::Deserialize;
use sha2::{Digest, Sha256};

/// The committed source of truth, embedded at build time.
pub const SANDBOX_LIBS_TOML: &str = include_str!("../sandbox-libs.toml");
/// The committed `uv pip compile --generate-hashes` lock for the toml.
pub const SANDBOX_LIBS_LOCK: &str = include_str!("../sandbox-libs.lock");

/// The planted probe tool's python source (R6): opens a 3-page PDF, reads a
/// 100-row xlsx and resizes a ~1 MB PNG to 256 px, all on the base env.
pub const PROBE_SOURCE: &str = include_str!("../sandbox-libs-probe.py");

/// The probe tool's spec, ready for `host.spec_test` / `host.tool_publish`.
pub fn probe_spec() -> serde_json::Value {
    serde_json::json!({
        "source": PROBE_SOURCE,
        "network": "none",
        "args_schema": {"type": "object"},
    })
}

/// Where the base env's `du -sm` must stay (R5).
pub const BASE_ENV_MAX_MB: u64 = 150;

#[derive(Debug, Clone, Deserialize)]
pub struct Lib {
    pub name: String,
    pub version: String,
    #[serde(rename = "import")]
    pub import: String,
    pub purpose: String,
    pub wheel_sha256: String,
    /// Dotted submodule names known to exist (optional; only used to catch
    /// a typo like `PIL.Imag`).
    #[serde(default)]
    pub modules: Vec<String>,
}

#[derive(Deserialize)]
struct Doc {
    lib: Vec<Lib>,
}

/// Parses `sandbox-libs.toml` text.
pub fn parse_libs(toml_text: &str) -> Result<Vec<Lib>, String> {
    toml::from_str::<Doc>(toml_text).map(|d| d.lib).map_err(|e| e.to_string())
}

/// The embedded rows.
pub fn libs() -> &'static [Lib] {
    static CELL: OnceLock<Vec<Lib>> = OnceLock::new();
    CELL.get_or_init(|| parse_libs(SANDBOX_LIBS_TOML).expect("sandbox-libs.toml must parse"))
}

/// One pinned entry of a `uv` lock: `(name, version, hashes)`.
pub fn parse_lock(lock_text: &str) -> Vec<(String, String, Vec<String>)> {
    let mut out: Vec<(String, String, Vec<String>)> = Vec::new();
    for line in lock_text.lines() {
        let line = line.trim();
        if let Some(hash) = line.strip_prefix("--hash=sha256:") {
            if let Some(last) = out.last_mut() {
                last.2.push(hash.trim_end_matches(['\\', ' ']).to_string());
            }
        } else if !line.is_empty()
            && !line.starts_with('#')
            && let Some((name, rest)) = line.split_once("==")
        {
            let version = rest.trim_end_matches(['\\', ' ']);
            out.push((name.trim().to_string(), version.to_string(), Vec::new()));
        }
    }
    out
}

/// PEP 503 normalisation: lowercase, runs of `-_.` -> `-`.
pub fn normalize(name: &str) -> String {
    let mut out = String::new();
    for c in name.trim().chars() {
        let c = if matches!(c, '_' | '.') { '-' } else { c.to_ascii_lowercase() };
        if c == '-' && out.ends_with('-') {
            continue;
        }
        out.push(c);
    }
    out
}

/// Hex sha256 of the lock text: the base env's directory name.
pub fn lock_hash() -> String {
    let digest = Sha256::digest(SANDBOX_LIBS_LOCK.as_bytes());
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

/// The five preinstalled package names, in toml order.
pub fn names() -> Vec<&'static str> {
    libs().iter().map(|l| l.name.as_str()).collect()
}

/// R2: true when `requirements` is non-empty and every entry names a
/// preinstalled lib, bare or pinned (`==`) to exactly the toml version.
pub fn requirements_are_preinstalled(requirements: &[String]) -> bool {
    !requirements.is_empty()
        && requirements.iter().all(|req| {
            let req = req.split(';').next().unwrap_or(req).trim();
            let (name, pin) = match req.split_once("==") {
                Some((n, v)) => (n.trim(), Some(v.trim())),
                None => (req, None),
            };
            libs().iter().any(|l| {
                normalize(&l.name) == normalize(name) && pin.is_none_or(|v| v == l.version)
            })
        })
}

/// The lib whose import name is exactly `top_level`.
pub fn lib_for_import(top_level: &str) -> Option<&'static Lib> {
    libs().iter().find(|l| l.import == top_level)
}

fn preinstalled_hint(lib: &Lib, suggestion: &str) -> String {
    format!("{} is preinstalled ({} {}): import {}", lib.import, lib.name, lib.version, suggestion)
}

/// R4: the closest listed import (or known dotted submodule) within edit
/// distance 2 of `dotted`, as a `<import> is preinstalled (...)` hint.
/// `None` when `dotted` is itself valid or nothing is close.
pub fn typo_hint(dotted: &str) -> Option<String> {
    let top = dotted.split('.').next().unwrap_or(dotted);
    if let Some(lib) = lib_for_import(top) {
        if lib.modules.is_empty() || dotted == top || lib.modules.iter().any(|m| m == dotted) {
            return None;
        }
        let (dist, best) = lib
            .modules
            .iter()
            .map(|m| (crate::errors::levenshtein(dotted, m), m))
            .min_by_key(|(d, _)| *d)?;
        return (dist <= 2).then(|| preinstalled_hint(lib, best));
    }
    if top.len() < 4 {
        return None;
    }
    let (dist, lib) = libs()
        .iter()
        .map(|l| (crate::errors::levenshtein(&top.to_ascii_lowercase(), &l.import.to_ascii_lowercase()), l))
        .min_by_key(|(d, _)| *d)?;
    (dist <= 2).then(|| preinstalled_hint(lib, &lib.import))
}

/// R4: hint for an import the tool declares no requirement for.
pub fn not_preinstalled_hint(module: &str) -> String {
    format!(
        "not preinstalled; add requirements: [\"{module}\"] or use one of: {}",
        names().join(", ")
    )
}

/// R1/R3 drift check over explicit texts: every toml `(name, version)` is in
/// the lock, every lock entry has a hash, each toml `wheel_sha256` is a lock
/// hash of its package, and both docs carry `render_sandbox_libs(toml)` byte
/// for byte. `Err` names the first mismatch.
pub fn check_drift(toml_text: &str, lock_text: &str, python_md: &str, llms_txt: &str) -> Result<(), String> {
    let rows = parse_libs(toml_text)?;
    let lock = parse_lock(lock_text);
    for l in &rows {
        let Some(entry) = lock
            .iter()
            .find(|(n, v, _)| normalize(n) == normalize(&l.name) && *v == l.version)
        else {
            return Err(format!("toml {} {} is not in sandbox-libs.lock", l.name, l.version));
        };
        if !entry.2.contains(&l.wheel_sha256) {
            return Err(format!("{}: wheel_sha256 is not one of the lock's hashes", l.name));
        }
    }
    for (name, version, hashes) in &lock {
        if hashes.is_empty() {
            return Err(format!("lock entry {name}=={version} has no hash"));
        }
    }
    let region = crate::gendocs::render_sandbox_libs(toml_text);
    for (label, doc) in [("docs/kinds/python.md", python_md), ("www/llms.txt", llms_txt)] {
        let start = doc.find(crate::gendocs::SANDBOX_LIBS_START);
        let end = doc.find(crate::gendocs::SANDBOX_LIBS_END);
        match (start, end) {
            (Some(s), Some(e)) if e > s => {
                if doc[s..e + crate::gendocs::SANDBOX_LIBS_END.len()] != region {
                    return Err(format!("{label}: sandbox-libs table differs from render_sandbox_libs"));
                }
            }
            _ => return Err(format!("{label}: missing sandbox-libs markers")),
        }
    }
    Ok(())
}
