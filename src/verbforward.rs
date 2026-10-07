//! PRD-mcphost-tool-call-host-verb-forward: the normalizer and allow/deny
//! lists `handler.rs`'s `host.tool_call`/`host.tool_test` forwarding and
//! their own hint-parity test share -- "one pure function" per the PRD's
//! own technical considerations, so a hint that names a verb and the
//! dispatch path that resolves it can never drift apart.

use crate::kinds::KindRegistry;

/// Requirement 1: every `host.*` verb except these four is forwardable --
/// `host.tool.call`/`host.tool.test` would recurse straight back into this
/// very dispatch, `host.key.rotate`/`host.self.offboard` are destructive
/// control-plane actions a misrouted hint must never reach by accident.
/// Named by canonical dotted form, the same strings `handler::host_tools`/
/// `handler::dispatch_tenant_tool` already dispatch on.
pub const FORWARD_DENYLIST: &[&str] = &[
    "host.tool.call",
    "host.tool.test",
    "host.key.rotate",
    "host.self.offboard",
];

/// Requirement 3 (AC5): host verbs whose entire effect is one or more
/// `state.db.*` writes, so wrapping their dispatch in the same savepoint
/// `dryrun::with_dry_run` scope `host.tool_test` already uses for a
/// published tool's `kind.call` rolls them back cleanly -- no outbound
/// call, no job enqueue, nothing a background task could act on before the
/// rollback lands. Every other host verb has no dry run yet: `host.tool_test`
/// on it returns `unverifiable` instead of forwarding a real write.
pub const DRY_RUN_FORWARDABLE: &[&str] = &["host.trigger.set"];

/// Folds a wire-spelled verb name to a form comparable against a canonical
/// dotted name regardless of which separator (`.` or `_`) the caller used
/// at each position, and regardless of whether they included the leading
/// `host.`/`host_` -- "host.trigger.set", "trigger.set", "host_trigger_set",
/// and "trigger_set" all fold to "triggerset".
fn fold(name: &str) -> String {
    let rest = name
        .strip_prefix("host.")
        .or_else(|| name.strip_prefix("host_"))
        .unwrap_or(name);
    rest.chars().filter(|c| *c != '.' && *c != '_').collect()
}

/// Requirement 1: `name`, in any spelling, resolved to the one real
/// canonical dotted host verb it names -- `None` for an `admin.*`/
/// `billing.*` name (Non-goals: never forwarded, whatever the registry
/// says -- callers check that prefix themselves before ever reaching here)
/// and for a name that doesn't fold-match any verb `handler::host_tools`
/// actually dispatches.
pub fn resolve(name: &str, kinds: &KindRegistry) -> Option<String> {
    if name.starts_with("admin.") || name.starts_with("billing.") {
        return None;
    }
    let folded = fold(name);
    crate::handler::host_tool_descriptors(kinds)
        .into_iter()
        .map(|t| t.name.to_string())
        // `host_tool_descriptors` lists both a verb's canonical dotted name
        // and (PRD-mcphost-tool-naming-convention-and-aliases) each of its
        // registered underscore aliases as separate entries -- an alias
        // folds identically to its own canonical target by construction, so
        // without this filter `resolve` could return the alias spelling
        // itself, which `dispatch_tenant_tool`'s match (dotted names only)
        // would never match. Same filter `dispatch_tenant_tool`'s own
        // fallback candidates list already uses for the identical reason.
        .filter(|n| crate::tool_aliases::resolve(n).is_none())
        .find(|n| n.starts_with("host.") && fold(n) == folded)
}

/// The client-visible (underscore) name of a canonical dotted verb, e.g.
/// `host.trigger.set` -> `host_trigger_set` -- every forward/refusal's own
/// `client_tool`/`call_instead.tool`.
pub fn client_tool_name(canonical: &str) -> String {
    canonical.replace('.', "_")
}

/// `true` iff `canonical` (already [`resolve`]d) is one of the four verbs
/// this host never forwards through `host.tool_call`/`host.tool_test`.
pub fn is_denylisted(canonical: &str) -> bool {
    FORWARD_DENYLIST.contains(&canonical)
}

/// `true` iff `canonical` (already [`resolve`]d) has a dry run
/// `host.tool_test` can forward to.
pub fn supports_dry_run(canonical: &str) -> bool {
    DRY_RUN_FORWARDABLE.contains(&canonical)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kinds::KindRegistry;

    fn kinds() -> KindRegistry {
        KindRegistry::with_builtin()
    }

    #[test]
    fn every_spelling_of_trigger_set_folds_identically() {
        let k = kinds();
        for spelling in ["host.trigger.set", "trigger.set", "host_trigger_set", "trigger_set"] {
            assert_eq!(
                resolve(spelling, &k).as_deref(),
                Some("host.trigger.set"),
                "spelling {spelling} must resolve to host.trigger.set"
            );
        }
    }

    #[test]
    fn admin_and_billing_never_resolve() {
        let k = kinds();
        assert_eq!(resolve("admin.tenants_list", &k), None);
        assert_eq!(resolve("billing.status", &k), None);
    }

    #[test]
    fn denylisted_verbs_still_resolve_but_are_flagged() {
        let k = kinds();
        let canonical = resolve("host_tool_call", &k).expect("host_tool_call resolves");
        assert_eq!(canonical, "host.tool.call");
        assert!(is_denylisted(&canonical));
    }

    #[test]
    fn unknown_name_resolves_to_none() {
        let k = kinds();
        assert_eq!(resolve("totally.unknown_verb", &k), None);
    }

    #[test]
    fn no_two_registered_verbs_fold_to_the_same_value() {
        let k = kinds();
        let names: Vec<String> = crate::handler::host_tool_descriptors(&k)
            .into_iter()
            .map(|t| t.name.to_string())
            .filter(|n| n.starts_with("host.") && crate::tool_aliases::resolve(n).is_none())
            .collect();
        let mut folded: Vec<String> = names.iter().map(|n| fold(n)).collect();
        folded.sort();
        let mut deduped = folded.clone();
        deduped.dedup();
        assert_eq!(
            folded.len(),
            deduped.len(),
            "two distinct host verbs fold to the same normalized name: {names:?}"
        );
    }
}
