//! PRD-mcphost-tool-naming-convention-and-aliases: the single source for
//! the `host.*` naming rule's alias table, read by four consumers (the
//! registry in `handler.rs`, `tools/list`'s `x-deprecated` metadata,
//! dispatch's alias resolution, and `scripts/tool-naming-lint.sh`'s own
//! mirror of this list) -- see the PRD's Technical considerations.
//!
//! Rule: `host.<family>.<verb>` for everything under `host`; `signup` and
//! `billing.*` are documented top-level exceptions (they predate the
//! namespace); `admin.*` is out of scope entirely (operator-only, a
//! separate rule -- PRD non-goals). A handful of single-word tools with no
//! sibling verb under their own name keep `host.<noun>` rather than being
//! split into a trivial `host.<noun>.<noun>` -- see
//! [`SINGLETON_NOUN_EXCEPTIONS`].

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use serde_json::{Map, Value, json};

/// requirement 2: every alias's lead time. Fixed to this PRD's landing
/// date rather than computed from `now_unix()` at each `tools/list` call,
/// so "90 days after landing" never drifts per request -- see
/// [`ALIAS_SUNSET_DATE`].
pub const ALIAS_LANDING_DATE: &str = "2026-10-01";

/// `ALIAS_LANDING_DATE` + 90 days, computed by hand once (this crate has
/// no calendar-math dependency): Oct has 31 days, so Oct 1 + 90d lands on
/// Dec 30 (30 remaining days of Oct + 30 of Nov + 29 of Dec + the 1st day
/// = 90). AC3's `sunset` for every alias.
pub const ALIAS_SUNSET_DATE: &str = "2026-12-30";

/// `(alias, canonical)` for every `host.*` tool name that violated the
/// dotted rule at this PRD's landing (requirement 3). Each alias dispatches
/// to the same handler as its canonical ([`canonicalize`]) and is
/// advertised in `tools/list` with `_meta["x-deprecated"]` ([`x_deprecated_map`]).
pub const ALIASES: &[(&str, &str)] = &[
    ("host.key_rotate", "host.key.rotate"),
    ("host.self_offboard", "host.self.offboard"),
    ("host.tool_publish", "host.tool.publish"),
    ("host.tool_list", "host.tool.list"),
    ("host.tool_remove", "host.tool.remove"),
    ("host.tool_logs", "host.tool.logs"),
    ("host.tool_test", "host.tool.test"),
    ("host.bridge_test", "host.bridge.test"),
    ("host.tool_run", "host.tool.run"),
    ("host.tool_call", "host.tool.call"),
    ("host.tool_history", "host.tool.history"),
    ("host.tool_rollback", "host.tool.rollback"),
    ("host.tool_diff", "host.tool.diff"),
    ("host.tool_share", "host.tool.share"),
    ("host.tool_spec_shared", "host.tool.spec_shared"),
    ("host.tool_unshare", "host.tool.unshare"),
    ("host.secret_set", "host.secret.set"),
    ("host.secret_list", "host.secret.list"),
    ("host.registry_publish", "host.registry.publish"),
    ("host.spec_test", "host.spec.test"),
];

/// requirement 3's "YES -- NO" carve-out: a tool that is itself a single
/// word, with no sibling verb registered under the same family, keeps
/// `host.<noun>` rather than being forced into `host.<noun>.<noun>`.
/// `scripts/tool-naming-lint.sh` mirrors this exact list (and the
/// `host.state.*`/`host.table.*`/... family prefixes already compliant)
/// so a new violator in either style is caught the same way.
pub const SINGLETON_NOUN_EXCEPTIONS: &[&str] = &[
    "host.whoami",
    "host.redeem",
    "host.quickstart",
    "host.usage",
    "host.changelog",
    "host.export",
    "host.progress",
];

/// Top-level names that sit outside `host.*` entirely by design (PRD
/// TL;DR): `signup` predates the namespace, `billing.*` is its own
/// documented family. Neither is a naming-rule violation.
pub const TOP_LEVEL_EXCEPTIONS: &[&str] = &["signup"];

/// A top-level family prefix excused from the `host.*` rule, alongside
/// [`TOP_LEVEL_EXCEPTIONS`]'s bare names.
pub const EXCEPTION_FAMILY_PREFIXES: &[&str] = &["billing."];

/// `admin.*` is out of scope for this PRD (operator-only surface, a
/// separate naming rule per the PRD's own non-goals) -- never a violation,
/// never aliased, never linted by `tool-naming-lint.sh`.
pub const OUT_OF_SCOPE_PREFIX: &str = "admin.";

/// Resolve `name` to its canonical form (requirement 2: "dispatch resolves
/// aliases before lookup"). Returns `name` unchanged when it is not a
/// registered alias -- including when it is already canonical.
pub fn canonicalize(name: &str) -> &str {
    ALIASES.iter().find(|(alias, _)| *alias == name).map_or(name, |(_, canonical)| *canonical)
}

/// The canonical name `name` is an alias for, or `None` when `name` is not
/// a registered alias (including when it's already canonical, or unknown
/// entirely).
pub fn canonical_for_alias(name: &str) -> Option<&'static str> {
    ALIASES.iter().find(|(alias, _)| *alias == name).map(|(_, canonical)| *canonical)
}

/// AC3: the `_meta["x-deprecated"]` object an alias [`rmcp::model::Tool`]
/// descriptor carries in `tools/list` -- a sibling of `inputSchema`
/// (`_meta` is the protocol's own extension point, `rmcp::model::MetaObject`),
/// never nested inside it, so the alias's `inputSchema` stays byte-identical
/// to its canonical's as AC3 requires.
pub fn x_deprecated_map(canonical: &str) -> Map<String, Value> {
    let mut m = Map::new();
    m.insert(
        "x-deprecated".to_string(),
        json!({"replaced_by": canonical, "sunset": ALIAS_SUNSET_DATE}),
    );
    m
}

/// requirement 2's "at most once per tenant per day" cap on the
/// `tool_deprecated_alias` log line: `(tenant_id, alias)` -> the unix-day
/// it was last logged. In-memory only, same `Arc<Mutex<HashMap<_, _>>>`
/// shape as [`crate::billing::AcceptedUsageCache`] -- a restart re-logging
/// once more costs nothing (this backs a log line and a counter, not a
/// correctness-bearing decision).
pub type AliasLogDedupe = Arc<Mutex<HashMap<(i64, String), i64>>>;

pub fn new_alias_log_dedupe() -> AliasLogDedupe {
    Arc::new(Mutex::new(HashMap::new()))
}

/// `true` (and records it) the first time `(tenant_id, alias)` is seen on
/// unix-day `now_unix / 86_400`; `false` on every later call the same day.
/// A poisoned lock is treated as "log it" (recovers the guard rather than
/// panicking -- a lost dedup entry just means one extra log line, never a
/// dropped call).
pub fn should_log_alias_use(cache: &AliasLogDedupe, tenant_id: i64, alias: &str, now_unix: i64) -> bool {
    let day = now_unix / 86_400;
    let mut map = cache.lock().unwrap_or_else(|p| p.into_inner());
    let key = (tenant_id, alias.to_string());
    if map.get(&key) == Some(&day) {
        false
    } else {
        map.insert(key, day);
        true
    }
}

/// requirement 6 (P1, AC7): sessions that have already been shown
/// `naming_rule_url` on `host.whoami`. In-memory only, same rationale as
/// [`AliasLogDedupe`].
pub type NamingRuleUrlSeen = Arc<Mutex<std::collections::HashSet<String>>>;

pub fn new_naming_rule_url_seen() -> NamingRuleUrlSeen {
    Arc::new(Mutex::new(std::collections::HashSet::new()))
}

/// `true` the first time `session_id` calls `host.whoami` in this
/// process's lifetime, `false` every later call -- AC7's "the second call
/// in the session does not" (carry `naming_rule_url`). A caller with no
/// session id (no `Mcp-Session-Id` negotiated) is treated as always-first:
/// there is nothing to dedup against, and showing the hint every time is
/// the safer default over never showing it.
pub fn first_naming_rule_url_in_session(cache: &NamingRuleUrlSeen, session_id: Option<&str>) -> bool {
    let Some(session_id) = session_id else {
        return true;
    };
    let mut seen = cache.lock().unwrap_or_else(|p| p.into_inner());
    seen.insert(session_id.to_string())
}

/// requirement 6: the doc a caller that guessed an old-style name can
/// self-correct from. The repo's own path rather than a `mcphost.dev`
/// route, since `docs/*.md` is not guaranteed to be served live by every
/// deployment (see the 2026-09-30 audit's README-link findings) while the
/// public repo always resolves.
pub const NAMING_RULE_URL: &str = "https://github.com/j0yen/mcphost/blob/main/docs/tool-naming.md";

/// The canonical name `name` is tracked under, whether `name` is itself
/// that canonical or one of its aliases -- `None` for any name outside
/// this table entirely. Used by the requirement-5 call counters below,
/// which bucket by canonical name regardless of which spelling a caller
/// used.
pub fn canonical_tracked(name: &str) -> Option<&'static str> {
    if let Some(c) = canonical_for_alias(name) {
        return Some(c);
    }
    ALIASES.iter().find(|(_, canonical)| *canonical == name).map(|(_, c)| *c)
}

/// requirement 5: per-canonical-name call counts, split by whether the
/// caller used the alias or the canonical spelling -- so the sunset
/// decision (removing an alias once usage reaches zero) has a number
/// behind it. In-memory only (reset on restart, same posture as
/// `oauth_stats`'s own healthz cache) -- counts every successful call,
/// keyed by canonical name, value `(alias_calls, canonical_calls)`.
pub type AliasCallCounters = Arc<Mutex<HashMap<&'static str, (u64, u64)>>>;

pub fn new_alias_call_counters() -> AliasCallCounters {
    let mut map = HashMap::with_capacity(ALIASES.len());
    for (_, canonical) in ALIASES {
        map.insert(*canonical, (0u64, 0u64));
    }
    Arc::new(Mutex::new(map))
}

/// Record one successful call of `name` (whichever spelling a caller
/// used) against its tracked canonical, incrementing the alias or
/// canonical half of that canonical's pair accordingly. A no-op for any
/// name [`canonical_tracked`] doesn't recognize.
pub fn record_call(counters: &AliasCallCounters, name: &str) {
    let Some(canonical) = canonical_tracked(name) else {
        return;
    };
    let used_alias = name != canonical;
    let mut map = counters.lock().unwrap_or_else(|p| p.into_inner());
    let entry = map.entry(canonical).or_insert((0, 0));
    if used_alias {
        entry.0 += 1;
    } else {
        entry.1 += 1;
    }
}

/// `/healthz`'s `tool_aliases` field (requirement 5, AC5): `{<canonical>:
/// {"alias": <n>, "canonical": <n>}, ...}` for every tracked canonical,
/// present even at zero so an operator can see which aliases have truly
/// gone quiet.
pub fn healthz_json(counters: &AliasCallCounters) -> Value {
    let map = counters.lock().unwrap_or_else(|p| p.into_inner());
    let mut out = Map::new();
    for (canonical, (alias_calls, canonical_calls)) in map.iter() {
        out.insert((*canonical).to_string(), json!({"alias": alias_calls, "canonical": canonical_calls}));
    }
    Value::Object(out)
}

/// AC6's `did_you_mean`: edit-distance-1 (requirement wording: "one edit
/// away") matches against every name this PRD's own table governs --
/// both sides of [`ALIASES`], since a typo of either an old or a new name
/// is equally plausible from a caller mid-migration. Scoped to this
/// table (not the full ~190-tool registry) rather than threading
/// `KindRegistry` through [`crate::errors::AppError::into_error_data`],
/// which has no such access today and whose call sites (dozens, several
/// with no `AppState` in scope) would all need to grow one for a single
/// advisory hint -- the smallest change that serves the AC's own example
/// (`host.tool.shar` -> `host.tool.share`).
pub fn did_you_mean(requested: &str) -> Vec<&'static str> {
    let mut candidates: Vec<&'static str> = Vec::with_capacity(ALIASES.len() * 2);
    for (alias, canonical) in ALIASES {
        candidates.push(alias);
        candidates.push(canonical);
    }
    candidates.sort_unstable();
    candidates.dedup();
    let mut scored: Vec<(usize, &'static str)> = candidates
        .into_iter()
        .map(|c| (crate::errors::levenshtein(requested, c), c))
        .filter(|(d, _)| *d <= 1 && *d > 0)
        .collect();
    scored.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(b.1)));
    scored.into_iter().map(|(_, c)| c).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonicalize_resolves_every_alias_and_passes_through_unknown_names() {
        for (alias, canonical) in ALIASES {
            assert_eq!(canonicalize(alias), *canonical);
        }
        assert_eq!(canonicalize("host.whoami"), "host.whoami");
        assert_eq!(canonicalize("host.tool.publish"), "host.tool.publish");
    }

    #[test]
    fn canonical_for_alias_is_none_for_a_canonical_or_unknown_name() {
        assert_eq!(canonical_for_alias("host.tool_publish"), Some("host.tool.publish"));
        assert_eq!(canonical_for_alias("host.tool.publish"), None);
        assert_eq!(canonical_for_alias("host.whoami"), None);
    }

    #[test]
    fn should_log_alias_use_is_true_once_per_tenant_per_day() {
        let cache = new_alias_log_dedupe();
        let day0 = 0i64;
        assert!(should_log_alias_use(&cache, 1, "host.tool_share", day0));
        assert!(!should_log_alias_use(&cache, 1, "host.tool_share", day0 + 100));
        // A different tenant, or a different alias, gets its own slot.
        assert!(should_log_alias_use(&cache, 2, "host.tool_share", day0));
        assert!(should_log_alias_use(&cache, 1, "host.tool_publish", day0));
        // A day later, the same (tenant, alias) logs again.
        assert!(should_log_alias_use(&cache, 1, "host.tool_share", day0 + 86_400));
    }

    #[test]
    fn first_naming_rule_url_in_session_is_true_once_per_session() {
        let cache = new_naming_rule_url_seen();
        assert!(first_naming_rule_url_in_session(&cache, Some("sess-1")));
        assert!(!first_naming_rule_url_in_session(&cache, Some("sess-1")));
        assert!(first_naming_rule_url_in_session(&cache, Some("sess-2")));
        // No session id: always "first".
        assert!(first_naming_rule_url_in_session(&cache, None));
        assert!(first_naming_rule_url_in_session(&cache, None));
    }

    #[test]
    fn did_you_mean_finds_a_one_edit_typo_of_a_canonical_name() {
        assert_eq!(did_you_mean("host.tool.shar"), vec!["host.tool.share"]);
    }

    #[test]
    fn did_you_mean_is_empty_for_a_distant_name() {
        // Not "an exact name": host.tool.share is itself exactly one
        // edit from its own alias host.tool_share (the dot/underscore
        // swap), so did_you_mean("host.tool.share") correctly returns
        // that alias -- a real caller never hits this path on an exact
        // match anyway (the call would just succeed, never reaching
        // ToolNotFound). Only a genuinely distant name is empty.
        assert!(did_you_mean("totally_unrelated").is_empty());
    }
}
