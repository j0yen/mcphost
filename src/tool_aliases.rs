//! PRD-mcphost-tool-naming-convention-and-aliases: the alias -> canonical
//! table for every `host.*` tool whose pre-PRD name violated the
//! `host.<family>.<verb>` rule documented in `docs/tool-naming.md`, plus the
//! small bits of process-wide state (alias-call metrics, per-tenant-per-day
//! log dedup, per-session `host.whoami` hint dedup) the alias dispatch path
//! in `handler.rs` needs. One file, read by four consumers (requirement 3's
//! own "single source of truth" clause): dispatch (`handler::call_tool`'s
//! alias resolution), `tools/list` (`handler::host_tools`'s alias-entry
//! injection), docs generation (`gendocs`'s `docs/tools.md` renderer), and
//! the lint (`scripts/tool-naming-lint.sh` parses this file's own
//! `TOOL_ALIASES` table by regex, the same way `spec-fields-doc-check.sh`
//! parses `known_spec_fields()` -- see that script's own header doc for why
//! a parsed source beats a hand-copied second list).

use std::collections::{HashMap, HashSet};
use std::sync::{Mutex, OnceLock};

use serde_json::{Value, json};

/// Requirement 2: `sunset = landing date + 90 days`. Computed once, at
/// this PRD's own build time (open question "Sunset length: 90 days
/// default", resolved at build per the PRD's own instruction), not at
/// read time -- a fixed commitment, not a rolling window that would make
/// `tools/list`'s own `x-deprecated.sunset` change on every call.
pub const SUNSET_DATE: &str = "2026-12-31";

/// One alias registered in `handler::host_tools`, dispatched through to
/// `canonical`'s own handler.
pub struct ToolAlias {
    pub alias: &'static str,
    pub canonical: &'static str,
}

/// Requirement 3: every current violator of the `host.<family>.<verb>`
/// rule, old name kept as an alias of its new dotted canonical. Derived
/// from the registry audit in this PRD's own PR body (the `host.tool_*`
/// family plus six singly-violating tools) -- see `docs/tool-naming.md`
/// for the rule these are all exceptions *to* (every other `host.*` tool
/// not listed here already satisfied it, or is a documented exception in
/// `scripts/tool-naming-lint.sh`).
pub const TOOL_ALIASES: &[ToolAlias] = &[
    ToolAlias { alias: "host.key_rotate", canonical: "host.key.rotate" },
    ToolAlias { alias: "host.self_offboard", canonical: "host.self.offboard" },
    ToolAlias { alias: "host.tool_publish", canonical: "host.tool.publish" },
    ToolAlias { alias: "host.tool_list", canonical: "host.tool.list" },
    ToolAlias { alias: "host.tool_remove", canonical: "host.tool.remove" },
    ToolAlias { alias: "host.tool_logs", canonical: "host.tool.logs" },
    ToolAlias { alias: "host.tool_test", canonical: "host.tool.test" },
    ToolAlias { alias: "host.bridge_test", canonical: "host.bridge.test" },
    ToolAlias { alias: "host.tool_run", canonical: "host.tool.run" },
    ToolAlias { alias: "host.tool_call", canonical: "host.tool.call" },
    ToolAlias { alias: "host.tool_history", canonical: "host.tool.history" },
    ToolAlias { alias: "host.tool_rollback", canonical: "host.tool.rollback" },
    ToolAlias { alias: "host.tool_diff", canonical: "host.tool.diff" },
    ToolAlias { alias: "host.tool_share", canonical: "host.tool.share" },
    ToolAlias { alias: "host.tool_spec_shared", canonical: "host.tool.spec_shared" },
    ToolAlias { alias: "host.tool_unshare", canonical: "host.tool.unshare" },
    ToolAlias { alias: "host.secret_set", canonical: "host.secret.set" },
    ToolAlias { alias: "host.secret_list", canonical: "host.secret.list" },
    ToolAlias { alias: "host.registry_publish", canonical: "host.registry.publish" },
    ToolAlias { alias: "host.spec_test", canonical: "host.spec.test" },
];

/// `name` -> its canonical name, iff `name` is a registered alias.
pub fn resolve(name: &str) -> Option<&'static str> {
    TOOL_ALIASES.iter().find(|a| a.alias == name).map(|a| a.canonical)
}

/// `true` iff `name` is one of [`TOOL_ALIASES`]' canonical targets (so it
/// has at least one deprecated alias pointing at it) -- requirement 5's
/// "canonical calls" side of the alias-call metric.
pub fn is_canonical_with_aliases(name: &str) -> bool {
    TOOL_ALIASES.iter().any(|a| a.canonical == name)
}

/// [`TOOL_ALIASES`]'s own `&'static str` for `name`, iff it's one of its
/// canonical targets -- the static-lifetime twin of
/// [`is_canonical_with_aliases`], needed wherever the caller's own
/// `&str` (an arbitrary-lifetime wire name) can't stand in for a
/// `&'static str` map key (`ALIAS_CALL_COUNTS` below is keyed on
/// `&'static str`, same as [`TOOL_ALIASES`] itself).
fn canonical_static(name: &str) -> Option<&'static str> {
    TOOL_ALIASES.iter().find(|a| a.canonical == name).map(|a| a.canonical)
}

/// Every alias of `canonical`, in [`TOOL_ALIASES`]' own order.
pub fn aliases_of(canonical: &str) -> Vec<&'static str> {
    TOOL_ALIASES.iter().filter(|a| a.canonical == canonical).map(|a| a.alias).collect()
}

// ---- PRD-mcphost-tools-list-alias-truth: one registry --------------------

/// One row of the name registry: a canonical tool, every deprecated alias
/// pointing at it, and its flattened (`a_b_c`) form -- the name MCP clients
/// build from `mcp__<server>__<name>`. `tools/list`, the dispatcher, the
/// contract dump and the docs all read these rows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistryEntry {
    pub canonical: String,
    pub aliases: Vec<&'static str>,
    /// `None` when the canonical has no `.` to flatten (e.g. `signup`).
    pub flattened: Option<String>,
}

/// A flattened name that would shadow a different tool.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistryCollision {
    pub flattened: String,
    pub first: String,
    pub second: String,
}

impl std::fmt::Display for RegistryCollision {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "tool name collision: `{}` and `{}` both resolve to `{}`",
            self.first, self.second, self.flattened
        )
    }
}

impl std::error::Error for RegistryCollision {}

/// `host.tool.run` -> `host_tool_run`; `None` for a name with no `.`.
pub fn flattened_form(canonical: &str) -> Option<String> {
    canonical.contains('.').then(|| canonical.replace('.', "_"))
}

/// The single registry function: `(canonical, aliases[], flattened)` per
/// canonical name in `canonicals`. Fails when two tools claim one flattened
/// name (`a.b_c` vs `a_b.c`), or a flattened name equals another tool's
/// exact name or alias.
pub fn build_registry(canonicals: &[&str]) -> Result<Vec<RegistryEntry>, RegistryCollision> {
    // exact name (canonical or alias) -> owning canonical
    let mut owners: HashMap<String, String> = HashMap::new();
    for c in canonicals {
        owners.insert((*c).to_string(), (*c).to_string());
        for a in aliases_of(c) {
            owners.entry(a.to_string()).or_insert_with(|| (*c).to_string());
        }
    }
    let mut claimed: HashMap<String, String> = HashMap::new();
    let mut out = Vec::with_capacity(canonicals.len());
    for c in canonicals {
        let flattened = flattened_form(c);
        if let Some(f) = &flattened {
            if let Some(other) = owners.get(f).filter(|o| o.as_str() != *c) {
                return Err(RegistryCollision { flattened: f.clone(), first: other.clone(), second: (*c).to_string() });
            }
            if let Some(other) = claimed.insert(f.clone(), (*c).to_string())
                && other != *c
            {
                return Err(RegistryCollision { flattened: f.clone(), first: other, second: (*c).to_string() });
            }
        }
        out.push(RegistryEntry { canonical: (*c).to_string(), aliases: aliases_of(c), flattened });
    }
    Ok(out)
}

// ---- requirement 5: alias-call metrics -------------------------------

/// `{canonical: (alias_calls, canonical_calls)}`.
type AliasCallCounts = HashMap<&'static str, (u64, u64)>;

/// Process-wide, in-memory alias-call counts -- same shape and lifetime
/// tradeoff as `help::HELP_HITS` (see that module's own doc comment for
/// why a global, not an `AppState` field, is the right call here).
static ALIAS_CALL_COUNTS: OnceLock<Mutex<AliasCallCounts>> = OnceLock::new();

fn alias_call_counts() -> &'static Mutex<AliasCallCounts> {
    ALIAS_CALL_COUNTS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Requirement 5: called once per successful `host.*`/`billing.*`
/// control-plane dispatch with the wire name the caller actually used.
/// A no-op for any name that isn't part of the alias system (every
/// ordinary canonical-with-no-aliases tool, `signup`, `billing.*`, etc.).
pub fn record_call(wire_name: &str) {
    let mut guard = alias_call_counts().lock().unwrap_or_else(|e| e.into_inner());
    if let Some(canonical) = resolve(wire_name) {
        guard.entry(canonical).or_insert((0, 0)).0 += 1;
    } else if let Some(canonical) = canonical_static(wire_name) {
        guard.entry(canonical).or_insert((0, 0)).1 += 1;
    }
}

/// `/healthz`'s `tool_alias_calls` object (requirement 5: "`docs/metrics.md`
/// names the counter"): `{canonical: {"alias": n, "canonical": n}}` for
/// every canonical name that has seen at least one call (alias or
/// canonical) since this process started.
pub fn alias_metrics_snapshot() -> Value {
    let guard = alias_call_counts().lock().unwrap_or_else(|e| e.into_inner());
    json!(
        guard
            .iter()
            .map(|(name, (alias, canonical))| {
                (name.to_string(), json!({"alias": alias, "canonical": canonical}))
            })
            .collect::<serde_json::Map<String, Value>>()
    )
}

// ---- requirement 2: per-tenant-per-day `tool_deprecated_alias` log ----

/// `(tenant_id, alias, day)`.
type AliasLogKey = (i64, String, String);

/// Already-logged keys -- requirement 2's "at most once per tenant per
/// day" dedup, per alias so an operator's trend line (user story: "alias
/// calls per day trending to zero") stays meaningful per tool rather than
/// collapsing every alias a tenant happens to use into one line.
static ALIAS_LOG_SEEN: OnceLock<Mutex<HashSet<AliasLogKey>>> = OnceLock::new();

fn alias_log_seen() -> &'static Mutex<HashSet<AliasLogKey>> {
    ALIAS_LOG_SEEN.get_or_init(|| Mutex::new(HashSet::new()))
}

/// `true` the first time `(tenant_id, alias, day)` calls this, `false`
/// every time after -- the pure dedup check [`maybe_log_deprecated_alias`]
/// logs on, exposed separately so a test can prove "at most once per
/// tenant per day" deterministically (no `tracing` subscriber/interest-
/// cache race: this consolidated, ~1000-test binary runs many unrelated
/// tests concurrently, any of which can re-evaluate this exact callsite's
/// global interest cache mid-test -- see
/// `tests/sessbind_ac10_request_log_records_the_upgraded_status.rs`'s own
/// doc comment for the mechanics).
pub fn mark_alias_logged(tenant_id: i64, alias: &str, day: &str) -> bool {
    let key = (tenant_id, alias.to_string(), day.to_string());
    let mut seen = alias_log_seen().lock().unwrap_or_else(|e| e.into_inner());
    seen.insert(key)
}

/// Logs `tool_deprecated_alias` for `tenant_hash`/`alias`/`canonical` at
/// most once per `(tenant_id, alias, day)` -- `day` is a bare
/// `"YYYY-MM-DD"` (`state::date_from_unix`), so this resets naturally at
/// UTC midnight with no explicit expiry bookkeeping required.
pub fn maybe_log_deprecated_alias(
    tenant_id: i64,
    tenant_hash: &str,
    alias: &str,
    canonical: &str,
    day: &str,
) {
    if mark_alias_logged(tenant_id, alias, day) {
        tracing::info!(tenant = %tenant_hash, alias = %alias, canonical = %canonical, "tool_deprecated_alias");
    }
}

// ---- requirement 6 (P1, AC7): once-per-session `naming_rule_url` ------

/// Session ids that have already seen `naming_rule_url` on a
/// `host.whoami` response -- an in-memory set, same "process-lifetime,
/// not persisted" tradeoff as the dedup/metrics above (and as
/// `session_bind::SessionBindings`, which this deliberately does not
/// reuse: that map tracks *tenant* bindings with LRU eviction and a 24h
/// TTL for a very different purpose; this is a one-bit-per-session flag
/// with no tenant identity in it at all -- an anonymous session that
/// never signs up still only gets the hint once).
static WHOAMI_HINT_SHOWN: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();

fn whoami_hint_shown() -> &'static Mutex<HashSet<String>> {
    WHOAMI_HINT_SHOWN.get_or_init(|| Mutex::new(HashSet::new()))
}

/// `true` the first time `session_id` calls this (so the caller should
/// attach `naming_rule_url`), `false` every time after.
pub fn mark_whoami_hint_shown(session_id: &str) -> bool {
    let mut seen = whoami_hint_shown().lock().unwrap_or_else(|e| e.into_inner());
    seen.insert(session_id.to_string())
}

/// `host.whoami`'s `naming_rule_url` (requirement 6): the committed rule
/// doc, same `REPO_DOCS_BASE` convention every other generated doc link
/// in this crate already uses (see `help::doc_href`).
pub fn naming_rule_url() -> String {
    format!("{}/docs/tool-naming.md", crate::help::REPO_DOCS_BASE)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_finds_a_registered_alias() {
        assert_eq!(resolve("host.tool_share"), Some("host.tool.share"));
    }

    #[test]
    fn resolve_is_none_for_a_canonical_name() {
        assert_eq!(resolve("host.tool.share"), None);
    }

    #[test]
    fn every_alias_differs_from_its_canonical() {
        for a in TOOL_ALIASES {
            assert_ne!(a.alias, a.canonical, "{} should not alias itself", a.alias);
        }
    }

    #[test]
    fn no_duplicate_alias_entries() {
        let mut seen = HashSet::new();
        for a in TOOL_ALIASES {
            assert!(seen.insert(a.alias), "duplicate alias entry: {}", a.alias);
        }
    }
}
